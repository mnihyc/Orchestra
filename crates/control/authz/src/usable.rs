//! May a person use a resource, and within what: the rule of a thread's provider and profile, of the host and template an environment is made from, and what a turn and a binding need of resources, asked of the person who acts.
//!
//! A thread holds nothing by itself, so the question is never asked about a thread. It is asked
//! about a person each time one does something: whoever puts a thread on a provider and a profile,
//! the author of an input when it is sent and again when its turn starts, the thread's first owner
//! in the place of the author for an input nobody sent. The answer is the person's own use of the
//! resource: whether they own it, or the system's own and they administer the system, or its Use
//! row or its Manage row names them (a person, a group, a role of theirs, everyone) or names the
//! work in a place that the thread lies in, and within which options. Administering a resource is
//! not using it, and nothing asks which org the person or the thread is of.
//!
//! [`check_turn`] is the one call the thread engine makes at an input and at a turn's pre-flight.
//! It says whether the turn may go on; or what stops it for everybody; or, refused, everything
//! the person lacks, thing by thing, in the shape the refusals of the node side have.

use std::collections::HashMap;

use orch_access_contract::decisions::{Closed, MissingOptions, ObjectRef, Refusal, Through};
use orch_access_contract::pages::{Entry, Holder, Origin};
use orch_access_contract::permissions::{
    Arrangement, Options, Permission, ProviderUse, ResourceKind,
};
use orch_db::Conn;
use orch_db::queries::resources as rows;
use orch_types::{Id, Time};

use crate::engine::{Frame, Graph, Site};
use crate::nodes::{Page, archive_allows};
use crate::resources::{
    Batch, Lacking, Listing, Resource, assemble, read_into, site_of, use_permission,
};
use crate::{Act, Caller, Error, Policy, Principal, options};

/// What the settings say that decides about one resource, as the caller resolved them at its home.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourcePolicy {
    /// `sharing.allow_external` at the resource's home: whether its owners may themselves name
    /// somebody from outside its root org on its page.
    pub owners_name_outsiders: bool,
    /// `providers.subscription_sharing` at its home: the harness kinds whose subscriptions may be
    /// used by people other than the provider's owner. Left unset, every kind is shared.
    pub shared_subscriptions: Option<Vec<String>>,
}

impl ResourcePolicy {
    /// Whether the resource is a subscription that its home keeps to its first owner: the policy
    /// leaves its harness kind out.
    #[must_use]
    pub fn keeps_to_its_owner(&self, resource: &Resource) -> bool {
        match (&resource.subscription, &self.shared_subscriptions) {
            (Some(kind), Some(shared)) => !shared.iter().any(|shared| shared == kind),
            _ => false,
        }
    }

    /// The policy the decisions of the node side take.
    #[must_use]
    pub fn node_policy(&self) -> Policy {
        Policy {
            owners_name_outsiders: self.owners_name_outsiders,
            ..Policy::default()
        }
    }
}

/// The policies of the homes a call is about. Settings are read elsewhere and resolve differently
/// at each home, so the caller says what they come to for the homes it knows the call touches; a
/// home it does not list follows the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policies {
    default: ResourcePolicy,
    homes: HashMap<Id, ResourcePolicy>,
    sharing: HashMap<Option<Id>, Policy>,
}

impl Policies {
    /// The same policy at every home.
    #[must_use]
    pub fn everywhere(policy: ResourcePolicy) -> Self {
        Self {
            default: policy,
            homes: HashMap::new(),
            sharing: HashMap::new(),
        }
    }

    /// The policy at the home `home` is `policy`.
    #[must_use]
    pub fn at_home(mut self, home: Id, policy: ResourcePolicy) -> Self {
        self.homes.insert(home, policy);
        self
    }

    /// The policy at a resource's home: the default for a resource of the system.
    #[must_use]
    pub fn at(&self, home: Option<Id>) -> &ResourcePolicy {
        home.and_then(|home| self.homes.get(&home))
            .unwrap_or(&self.default)
    }

    /// Keeps the sharing policy resolved at a resource's home beside its use policy.
    #[must_use]
    pub fn with_sharing(mut self, home: Option<Id>, policy: Policy) -> Self {
        self.sharing.insert(home, policy);
        self
    }

    /// The sharing policy at a home, retaining the ordinary use policy when none was supplied.
    #[must_use]
    pub fn sharing_at(&self, home: Option<Id>) -> Policy {
        self.sharing
            .get(&home)
            .copied()
            .unwrap_or_else(|| self.at(home).node_policy())
    }
}

/// How an entry that names the work in a place lets a thread there run on a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Work {
    /// The entry.
    pub entry: Id,
    /// The place it names: a place the thread lies in, or the thread itself.
    pub place: Id,
    /// The page it stands on.
    pub from: Origin,
}

/// A person's use of a resource, and within what.
#[derive(Debug, Clone, PartialEq)]
pub struct Use {
    /// What their entries come to, combined as the options of a Use row combine; unlimited for an
    /// owner and for a system admin on a resource of the system.
    pub options: Options,
    /// How they hold the Use row themself: as an owner, as the system's admin on a resource of
    /// the system, by a higher level or by an entry that names them.
    pub through: Vec<Through>,
    /// The entries by which the work in a place lets a thread there run on the resource, whoever
    /// gives it input.
    pub work: Vec<Work>,
    /// The entry a turn of theirs is put down to for the provider's own reports; none for an owner.
    pub entry: Option<Id>,
    /// Every entry that lets them in: their own, those of a group or a role of theirs, and those
    /// of the work in a place, in the Use row and in the Manage row. What a cap on the entry is
    /// counted against.
    pub entries: Vec<Id>,
}

impl Use {
    /// What they may run on a provider.
    #[must_use]
    pub fn provider(&self) -> ProviderUse {
        options::of_provider(&self.options)
    }
}

/// Why a person has no use of a resource, when the resource is in a state that allows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// They may use it.
    None,
    /// It is a subscription that its home keeps to its first owner, and they are not. Nobody else
    /// can give it, an admin included.
    KeptToItsOwner,
}

/// The use of a resource that a person has, if they have any, apart from what state the resource
/// is in: `Ok(Err(why))` for none, `Ok(Ok(use))` for some.
///
/// `work` is the path of the thread the question is about (the nodes from its root org down to it),
/// where there is one: an entry that names the work in a place on it lets the thread run on the
/// resource, whoever gives it input. A token that is kept to a subtree reaches a resource only
/// through a node in it, so the thread's path is also what the token is held to.
pub fn access(
    frame: &Frame<'_>,
    resource: &Resource,
    site: &Site,
    work: Option<&[Id]>,
    kept_to_its_owner: bool,
) -> Result<Result<Use, Access>, Error> {
    let Some(permission) = use_permission(resource.kind) else {
        return Ok(Err(Access::None));
    };
    let principal = frame.principal();
    let caller = principal.caller();
    let mut site = site.clone();
    if let Some(path) = work {
        site.path = Some(path.to_vec());
    }
    if !caller.permits(permission)
        || !caller.reaches(site.path.as_deref())
        || principal.suspension().applies(&site.orgs)
    {
        return Ok(Err(Access::None));
    }
    let ways = frame.ways(&site, permission);
    if kept_to_its_owner {
        // Used by its first owner alone, as an owner: what the page names for anybody else counts
        // for nothing while the policy holds, an admin's override among it.
        if resource.first_owner != Some(principal.user()) {
            return Ok(Err(Access::KeptToItsOwner));
        }
        return Ok(if ways.contains(&Through::Owner) {
            Ok(Use {
                options: options::unlimited(permission),
                through: vec![Through::Owner],
                work: Vec::new(),
                entry: None,
                entries: Vec::new(),
            })
        } else {
            Err(Access::None)
        });
    }
    let in_use = frame.matching(&site, permission, work);
    let in_manage = manage_of(resource.kind)
        .map(|manage| frame.matching(&site, manage, None))
        .unwrap_or_default();
    let works: Vec<Work> = in_use
        .iter()
        .filter_map(|found| match found.entry.holder {
            Holder::Node { id } if work.is_some_and(|path| path.contains(&id)) => Some(Work {
                entry: found.entry.id,
                place: id,
                from: found.from,
            }),
            _ => None,
        })
        .collect();
    if ways.is_empty() && works.is_empty() {
        return Ok(Err(Access::None));
    }
    // An owner has no limit, and neither has anybody the Manage row names: the Use row's options do
    // not narrow the level above it.
    let unlimited = ways
        .iter()
        .any(|way| matches!(way, Through::Owner | Through::SystemAdmin))
        || !in_manage.is_empty();
    let combined = if unlimited {
        options::unlimited(permission)
    } else {
        options::combine(permission, in_use.iter().map(|found| found.entry))?
            .unwrap_or_else(|| options::unlimited(permission))
    };
    let entries: Vec<&Entry> = in_use
        .iter()
        .chain(in_manage.iter())
        .map(|found| found.entry)
        .collect();
    let owns = ways
        .iter()
        .any(|way| matches!(way, Through::Owner | Through::SystemAdmin));
    Ok(Ok(Use {
        options: combined,
        through: ways,
        work: works,
        entry: if owns {
            None
        } else {
            options::attribution(principal.user(), work.unwrap_or_default(), &entries)
        },
        entries: entries.iter().map(|entry| entry.id).collect(),
    }))
}

/// The permission that is the highest level of a resource's area, which holds its Use.
fn manage_of(kind: ResourceKind) -> Option<Permission> {
    match kind {
        ResourceKind::Provider => Some(Permission::ProviderManage),
        ResourceKind::Agent => Some(Permission::AgentManage),
        ResourceKind::Host => Some(Permission::HostManage),
        ResourceKind::Machine => Some(Permission::MachineManage),
        ResourceKind::Template => Some(Permission::TemplateManage),
        ResourceKind::McpServer => Some(Permission::McpManage),
        ResourceKind::Mount => Some(Permission::MountManage),
        ResourceKind::Skill => Some(Permission::SkillManage),
        ResourceKind::Environment => None,
    }
}

/// Why a resource cannot be used by anybody, whatever its page says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// It is switched off: its use is refused until it is switched on.
    Disabled,
    /// It was deleted.
    Deleted,
}

impl Resource {
    /// Why nobody can use it now, if so.
    #[must_use]
    pub fn unavailable(&self) -> Option<Unavailable> {
        if self.deleted {
            Some(Unavailable::Deleted)
        } else if self.disabled {
            Some(Unavailable::Disabled)
        } else {
            None
        }
    }
}

/// The answer to whether a person may use a resource, as the rule of the access model states it.
#[derive(Debug, Clone, PartialEq)]
pub enum Usable {
    /// They may, within these options.
    Yes(Box<Use>),
    /// Nobody may: it is switched off or deleted.
    Unavailable(Unavailable),
    /// It is a subscription kept to its first owner, and they are not.
    KeptToItsOwner,
    /// Their page gives them no use of it.
    No,
}

impl Usable {
    /// Their use, where they have it.
    #[must_use]
    pub fn granted(&self) -> Option<&Use> {
        match self {
            Self::Yes(granted) => Some(granted),
            _ => None,
        }
    }
}

/// Whether the person may use the resource `resource` for the work at `node`, which is the thread,
/// or for creating one the node that thread is made in, or the node whose settings name the
/// resource; of any org. The statements are the same for any resource.
pub async fn usable(
    conn: &mut Conn,
    principal: &Principal,
    resource: Id,
    node: Option<Id>,
    policies: &Policies,
) -> Result<Usable, Error> {
    let mut graph = Graph::default();
    if let Some(node) = node {
        graph.extend(conn, &[node]).await?;
    }
    let found = read_into(conn, &mut graph, &[resource]).await?;
    let resource = found
        .into_iter()
        .find(|found| found.id == resource)
        .ok_or(Error::NotFound(resource))?;
    let site = site_of(&graph, &resource).ok_or(Error::NotFound(resource.id))?;
    let path = node
        .and_then(|node| graph.get(node))
        .map(|node| node.path.clone());
    let policy = policies.at(resource.home);
    let frame = Frame::load(
        conn,
        principal,
        policy.node_policy(),
        graph,
        std::slice::from_ref(&site),
        &use_permission(resource.kind)
            .into_iter()
            .collect::<Vec<_>>(),
    )
    .await?;
    if let Some(unavailable) = resource.unavailable() {
        return Ok(Usable::Unavailable(unavailable));
    }
    Ok(
        match access(
            &frame,
            &resource,
            &site,
            path.as_deref(),
            policy.keeps_to_its_owner(&resource),
        )? {
            Ok(granted) => Usable::Yes(Box::new(granted)),
            Err(Access::KeptToItsOwner) => Usable::KeptToItsOwner,
            Err(Access::None) => Usable::No,
        },
    )
}

/// Of the pairs `pairs` (a thread, and the provider it runs on), those for which the person may
/// use the provider for that thread, as [`usable`] says it for each: the statements are those of
/// one question, whatever the number of threads. A provider that is switched off or deleted, or
/// that is not there, serves no thread.
pub async fn usable_on_threads(
    conn: &mut Conn,
    principal: &Principal,
    pairs: &[(Id, Id)],
    policies: &Policies,
) -> Result<Vec<(Id, Id)>, Error> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let mut threads: Vec<Id> = pairs.iter().map(|(thread, _)| *thread).collect();
    threads.sort_unstable();
    threads.dedup();
    let mut providers: Vec<Id> = pairs.iter().map(|(_, provider)| *provider).collect();
    providers.sort_unstable();
    providers.dedup();
    let mut graph = Graph::default();
    graph.extend(conn, &threads).await?;
    let found = read_into(conn, &mut graph, &providers).await?;
    let sites: Vec<Site> = found
        .iter()
        .filter_map(|resource| site_of(&graph, resource))
        .collect();
    let frame = Frame::load(
        conn,
        principal,
        policies.at(None).node_policy(),
        graph,
        &sites,
        &[Permission::ProviderUse],
    )
    .await?;
    let mut usable: Vec<(Id, Id)> = Vec::new();
    for (thread, provider) in pairs {
        let Some(resource) = found.iter().find(|resource| resource.id == *provider) else {
            continue;
        };
        let Some(site) = sites.iter().find(|site| site.id == *provider) else {
            continue;
        };
        let Some(path) = frame.graph().get(*thread).map(|node| node.path.clone()) else {
            continue;
        };
        if resource.unavailable().is_some() {
            continue;
        }
        let kept = policies.at(resource.home).keeps_to_its_owner(resource);
        if access(&frame, resource, site, Some(&path), kept)?.is_ok() {
            usable.push((*thread, *provider));
        }
    }
    Ok(usable)
}

/// A resource that a person may use, and within what.
#[derive(Debug, Clone, PartialEq)]
pub struct Usables {
    /// The resource.
    pub resource: Resource,
    /// Their use of it.
    pub granted: Use,
}

/// The resources of the kind `kind` that the person may use for the work at `node` (where they
/// stand: the topic they are in, the node whose settings are being read), one page of them in the
/// order of their identifiers, each with the options that apply to them there. A resource that is
/// switched off or deleted is not among them, and neither is one that is only looked at. For an
/// environment, putting a thread into it takes its Manage (its Use lets its threads' agents act for a
/// person, and is asked when they give input), so what may be used here is what is managed. The statements are those
/// of one batch of candidates however many there are.
pub async fn usable_list(
    conn: &mut Conn,
    principal: &Principal,
    kind: ResourceKind,
    node: Option<Id>,
    policies: &Policies,
    page: Page,
) -> Result<Listing<Usables>, Error> {
    let Page { after, limit } = page;
    let path = match node {
        Some(node) => {
            let graph = Graph::load(conn, &[node]).await?;
            Some(graph.get(node).ok_or(Error::NotFound(node))?.path.clone())
        }
        None => None,
    };
    let mut items: Vec<Usables> = Vec::new();
    let mut cursor = after;
    while items.len() <= limit {
        let batch = Batch::read(conn, principal, kind, cursor, Policy::default()).await?;
        for (resource, site) in &batch.resources {
            if resource.unavailable().is_some() {
                continue;
            }
            let policy = policies.at(resource.home);
            let granted = if kind == ResourceKind::Environment {
                let ways = batch.frame.ways(site, Permission::EnvironmentUse);
                (!ways.is_empty()).then(|| Use {
                    options: Options::None,
                    through: ways,
                    work: Vec::new(),
                    entry: None,
                    entries: Vec::new(),
                })
            } else {
                access(
                    &batch.frame,
                    resource,
                    site,
                    path.as_deref(),
                    policy.keeps_to_its_owner(resource),
                )?
                .ok()
            };
            if let Some(granted) = granted {
                items.push(Usables {
                    resource: resource.clone(),
                    granted,
                });
            }
        }
        cursor = batch.last;
        if batch.exhausted || batch.last.is_none() {
            break;
        }
    }
    let more = items.len() > limit;
    items.truncate(limit);
    Ok(Listing {
        next: if more {
            items.last().map(|item| item.resource.id)
        } else {
            None
        },
        items,
    })
}

/// Who a turn is asked about.
#[derive(Debug, Clone, Copy)]
pub enum Author<'a> {
    /// The author of the input: they need `thread.interact` on the thread, and their own use of
    /// the thread's provider with its model and effort.
    Person(&'a Principal),
    /// An input no person sent (an announcement, something another agent sent, a notice of Orch's
    /// own) runs on the use the thread's first owner has of the provider, at the moment given.
    /// Nothing is asked of the thread itself: whoever announced it needed their own permission.
    FirstOwner {
        /// The moment.
        now: Time,
    },
}

/// What a turn is on: the thread, and what it runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnAsk<'a> {
    /// The thread the input is for.
    pub thread: Id,
    /// The provider the thread runs on.
    pub provider: Id,
    /// The agent profile the thread runs through; its state is looked at, and not who may use it:
    /// nobody is asked about that again once the thread is bound.
    pub profile: Option<Id>,
    /// The thread's model, as the provider names it.
    pub model: &'a str,
    /// The thread's effort, where it sets one.
    pub effort: Option<&'a str>,
    /// The arrangement type the thread is bound in, where it has one.
    pub arrangement: Option<Arrangement>,
    /// The setting `models.allowed` resolved at the thread, where any scope sets it.
    pub models_allowed: Option<&'a [String]>,
}

/// Why a turn cannot go on for anybody, whoever gives the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// The provider is switched off.
    ProviderDisabled,
    /// The provider was deleted.
    ProviderDeleted,
    /// The agent profile is switched off.
    ProfileDisabled,
    /// The agent profile was deleted.
    ProfileDeleted,
    /// The model is no model of the provider that is enabled, or the thread's place does not allow
    /// it.
    ModelNotAllowed {
        /// The model.
        model: String,
    },
    /// The model does not list that effort.
    EffortNotAllowed {
        /// The effort.
        effort: String,
    },
    /// The arrangement type is not allowed by the provider's owners, or by the entry the turn goes
    /// by; the thread can be bound in one that still is.
    ArrangementNotAllowed {
        /// The arrangement.
        arrangement: Arrangement,
    },
}

/// What a person who may drive the turn comes to on the provider.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnGrant {
    /// What their entries let them run.
    pub options: ProviderUse,
    /// The entry the turn is put down to for the provider's own reports (`leases.entry_id`); none
    /// for an owner.
    pub entry: Option<Id>,
    /// The models they may run on the provider: the effective allowlist of the thread for them.
    pub models: Vec<String>,
    /// The efforts they may run on the thread's model.
    pub efforts: Vec<String>,
    /// The arrangement types in which work done through their entries may use the provider.
    pub arrangements: Vec<Arrangement>,
    /// How they hold the use.
    pub through: Vec<Through>,
}

/// Whether a turn may go on.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnDecision {
    /// It may.
    Allowed(Box<TurnGrant>),
    /// The person has what the turn needs, and nobody could run it as it is set: the thread is
    /// blocked, whoever gives it input.
    Blocked(Vec<Blocker>),
    /// The person lacks something: what, thing by thing, and who can give it. For an input no
    /// person sent it is the thread's first owner who lacks it, and the input waits.
    Refused(Box<Refusal>),
}

/// What a person chose a provider to be used with.
#[derive(Debug, Clone, Copy)]
struct Chosen<'a> {
    model: &'a str,
    effort: Option<&'a str>,
    arrangement: Option<Arrangement>,
    models_allowed: Option<&'a [String]>,
}

/// What a person's own use of a provider comes to for the model and effort they chose.
struct ProviderVerdict {
    /// What they lack, if anything.
    lacking: Vec<Lacking>,
    /// What stops it for everybody, when they lack nothing.
    blockers: Vec<Blocker>,
    /// What they come to, when nothing stops it.
    grant: Option<TurnGrant>,
}

/// What is judged of a provider: where it lies, the work it is to run and what it is to run.
#[derive(Clone, Copy)]
struct Judging<'a> {
    provider: &'a Resource,
    site: &'a Site,
    /// The path of the thread or of the place the work is in.
    path: &'a [Id],
    chosen: &'a Chosen<'a>,
    /// Whether its home keeps it to its first owner.
    kept: bool,
    /// Whether the state of the provider and the models is looked at: only for somebody who lacks
    /// nothing else, so that nobody learns of it who would be refused in any case.
    state_matters: bool,
}

/// Judges the use of a provider for what was chosen, for the person the frame is about.
async fn judge_provider(
    conn: &mut Conn,
    frame: &Frame<'_>,
    judging: &Judging<'_>,
) -> Result<ProviderVerdict, Error> {
    let Judging {
        provider,
        site,
        path,
        chosen,
        kept,
        state_matters,
    } = *judging;
    let wanted_options = MissingOptions {
        models: Some(vec![chosen.model.to_owned()]),
        efforts: chosen.effort.map(|effort| vec![effort.to_owned()]),
    };
    let lack = |closed: Option<Closed>, options: MissingOptions| Lacking {
        site: site.clone(),
        permissions: vec![Permission::ProviderUse],
        options: Some(options),
        closed,
    };
    let mut verdict = ProviderVerdict {
        lacking: Vec::new(),
        blockers: Vec::new(),
        grant: None,
    };
    let granted = match access(frame, provider, site, Some(path), kept)? {
        Ok(granted) => granted,
        Err(Access::KeptToItsOwner) => {
            verdict
                .lacking
                .push(lack(Some(Closed::SubscriptionNotShared), wanted_options));
            return Ok(verdict);
        }
        Err(Access::None) => {
            verdict.lacking.push(lack(None, wanted_options));
            return Ok(verdict);
        }
    };
    if !state_matters {
        return Ok(verdict);
    }
    match provider.unavailable() {
        Some(Unavailable::Disabled) => verdict.blockers.push(Blocker::ProviderDisabled),
        Some(Unavailable::Deleted) => verdict.blockers.push(Blocker::ProviderDeleted),
        None => {}
    }
    let models = rows::provider_models(conn, provider.id).await?;
    let nobody = ProviderUse::default();
    let known = models.iter().find(|model| model.model_name == chosen.model);
    let for_anybody = options::allowed_models(&models, &nobody, chosen.models_allowed);
    if !for_anybody.contains(&chosen.model) {
        verdict.blockers.push(Blocker::ModelNotAllowed {
            model: chosen.model.to_owned(),
        });
    } else if let (Some(effort), Some(model)) = (chosen.effort, known)
        && !model.efforts.iter().any(|listed| listed == effort)
    {
        verdict.blockers.push(Blocker::EffortNotAllowed {
            effort: effort.to_owned(),
        });
    }
    let mine = granted.provider();
    let arrangements = options::allowed_arrangements(&provider.arrangements, &mine);
    if let Some(arrangement) = chosen.arrangement
        && !arrangements.contains(&arrangement)
    {
        verdict
            .blockers
            .push(Blocker::ArrangementNotAllowed { arrangement });
    }
    if !verdict.blockers.is_empty() {
        return Ok(verdict);
    }
    let my_models = options::allowed_models(&models, &mine, chosen.models_allowed);
    let my_efforts: Vec<&str> = known
        .map(|model| options::allowed_efforts(model, &mine))
        .unwrap_or_default();
    let narrowed_model = !my_models.contains(&chosen.model);
    let narrowed_effort = chosen
        .effort
        .is_some_and(|effort| !my_efforts.contains(&effort));
    if narrowed_model || narrowed_effort {
        verdict.lacking.push(lack(None, wanted_options));
    } else {
        verdict.grant = Some(TurnGrant {
            options: mine.clone(),
            entry: granted.entry,
            models: my_models.iter().map(|model| (*model).to_owned()).collect(),
            efforts: my_efforts
                .iter()
                .map(|effort| (*effort).to_owned())
                .collect(),
            arrangements,
            through: granted.through.clone(),
        });
    }
    Ok(verdict)
}

/// The refusal of something that a token's scopes leave out, which is for nobody to give.
fn out_of_scope(required: Permission, object: ObjectRef) -> Refusal {
    Refusal::of(required, object, Vec::new())
}

/// Whether the person may have the agents of the threads in the environment act for them: the Use row of the
/// environment's page or a level above it names them, or names the work in a place that the thread lies in
/// (`work`, the thread's path), which opens the environment to whoever may drive that work.
fn uses_environment(frame: &Frame<'_>, site: &Site, work: &[Id]) -> bool {
    let mut site = site.clone();
    // A token kept to a subtree reaches the environment only through the thread, as it reaches a provider.
    site.path = Some(work.to_vec());
    if !frame.ways(&site, Permission::EnvironmentUse).is_empty() {
        return true;
    }
    frame
        .matching(&site, Permission::EnvironmentUse, Some(work))
        .iter()
        .any(|found| matches!(found.entry.holder, Holder::Node { id } if work.contains(&id)))
}

/// Whether a turn on a thread may go on, for the person it is asked about: the one call the thread
/// engine makes when an input is sent and again when its turn starts.
///
/// The author needs `thread.interact` on the thread, their own use of the thread's provider
/// with its model and effort, and their own use of the environment the thread runs in, where its
/// agent acts for them; for an input without an author it is the thread's first owner who
/// needs the uses, in the author's place. The answer is [`TurnDecision::Refused`] with everything
/// the person lacks, thing by thing, and who can give each part, or [`TurnDecision::Blocked`]
/// when they lack nothing and the turn cannot run as it is set (the provider or the profile is
/// switched off or deleted, the model or the effort is not one, the arrangement type is not
/// allowed), or [`TurnDecision::Allowed`] with what the person's entries come to. Somebody whose
/// access is cut off altogether is refused before anything is said of the state of the resource,
/// so that no one learns of it who would be refused in any case.
///
/// A caller who is suspended where the thread or the provider is gets [`Error::Suspended`]; one
/// who holds what is needed but finds the thread archived gets [`Error::Archived`].
pub async fn check_turn(
    conn: &mut Conn,
    author: Author<'_>,
    ask: &TurnAsk<'_>,
    policies: &Policies,
) -> Result<TurnDecision, Error> {
    let mut graph = Graph::load(conn, &[ask.thread]).await?;
    let thread = graph
        .get(ask.thread)
        .cloned()
        .ok_or(Error::NotFound(ask.thread))?;
    let thread_site = graph
        .site_of(ask.thread)
        .ok_or(Error::NotFound(ask.thread))?;
    let environment = graph.environment_of_thread(ask.thread);
    let mut ids = vec![ask.provider];
    ids.extend(ask.profile);
    ids.extend(environment);
    let found = read_into(conn, &mut graph, &ids).await?;
    let provider = pick(&found, ask.provider, ResourceKind::Provider)?;
    let provider_site = site_of(&graph, &provider).ok_or(Error::NotFound(provider.id))?;
    let profile = ask
        .profile
        .map(|id| pick(&found, id, ResourceKind::Agent))
        .transpose()?;
    // An environment that cannot be read any more holds no thread's work: what runs there is stopped by
    // the binding's own checks, and nobody is asked for its use.
    let environment_site = environment
        .and_then(|id| pick(&found, id, ResourceKind::Environment).ok())
        .and_then(|environment| site_of(&graph, &environment));
    let first_owner;
    let (principal, authored): (&Principal, bool) = match author {
        Author::Person(principal) => (principal, true),
        Author::FirstOwner { now } => {
            first_owner = Principal::load(
                conn,
                Caller::session(thread.first_owner).with_admin_mode(false),
                now,
            )
            .await?;
            (&first_owner, false)
        }
    };
    let provider_object = provider.object();
    for site in [&thread_site, &provider_site]
        .into_iter()
        .chain(environment_site.as_ref())
    {
        if let Some(closing) = principal.suspension().closing(&site.orgs) {
            return if authored {
                Err(Error::Suspended { org: closing.org() })
            } else {
                // Their things stay theirs, but they use nothing where they are suspended: the
                // input waits for them.
                Ok(TurnDecision::Refused(Box::new(out_of_scope(
                    Permission::ProviderUse,
                    provider_object,
                ))))
            };
        }
    }
    let policy = policies.at(provider.home);
    let kept = policy.keeps_to_its_owner(&provider);
    let node_policy = policy.node_policy();
    let mut sites = vec![thread_site.clone(), provider_site.clone()];
    sites.extend(environment_site.clone());
    let wanted = [
        Permission::ThreadInteract,
        Permission::ProviderUse,
        Permission::EnvironmentUse,
    ];
    let frame = Frame::load(conn, principal, node_policy, graph, &sites, &wanted).await?;

    let mut lacking: Vec<Lacking> = Vec::new();
    if authored
        && frame
            .ways(&thread_site, Permission::ThreadInteract)
            .is_empty()
    {
        lacking.push(Lacking {
            site: thread_site.clone(),
            permissions: vec![Permission::ThreadInteract],
            options: None,
            closed: None,
        });
    }
    let chosen = Chosen {
        model: ask.model,
        effort: ask.effort,
        arrangement: ask.arrangement,
        models_allowed: ask.models_allowed,
    };
    let mut verdict = judge_provider(
        conn,
        &frame,
        &Judging {
            provider: &provider,
            site: &provider_site,
            path: &thread.path,
            chosen: &chosen,
            kept,
            state_matters: lacking.is_empty(),
        },
    )
    .await?;
    lacking.append(&mut verdict.lacking);
    if let Some(site) = &environment_site
        && !uses_environment(&frame, site, &thread.path)
    {
        lacking.push(Lacking {
            site: site.clone(),
            permissions: vec![Permission::EnvironmentUse],
            options: None,
            closed: None,
        });
    }
    let mut blockers = std::mem::take(&mut verdict.blockers);
    if lacking.is_empty()
        && let Some(profile) = &profile
    {
        match profile.unavailable() {
            Some(Unavailable::Disabled) => blockers.push(Blocker::ProfileDisabled),
            Some(Unavailable::Deleted) => blockers.push(Blocker::ProfileDeleted),
            None => {}
        }
    }
    if !lacking.is_empty() {
        let (required, anchor) = if authored {
            (Permission::ThreadInteract, &thread_site)
        } else {
            (Permission::ProviderUse, &provider_site)
        };
        if lacking
            .iter()
            .flat_map(|item| item.permissions.iter())
            .any(|permission| !principal.caller().permits(*permission))
        {
            // What a token's scopes leave out is not for anybody to give.
            return Ok(TurnDecision::Refused(Box::new(out_of_scope(
                required,
                ObjectRef {
                    kind: anchor.kind,
                    id: anchor.id,
                },
            ))));
        }
        let graph = frame.graph().clone();
        let refusal = assemble(
            conn,
            principal,
            node_policy,
            graph,
            anchor,
            &lacking,
            required,
        )
        .await?;
        return Ok(TurnDecision::Refused(Box::new(refusal)));
    }
    if authored
        && let Some(blocked) = thread_site.archived_node
        && !archive_allows(Permission::ThreadInteract, Act::Use)
    {
        return Err(Error::Archived { node: blocked });
    }
    if !blockers.is_empty() {
        return Ok(TurnDecision::Blocked(blockers));
    }
    match verdict.grant {
        Some(grant) => Ok(TurnDecision::Allowed(Box::new(grant))),
        None => Ok(TurnDecision::Refused(Box::new(out_of_scope(
            Permission::ProviderUse,
            provider_object,
        )))),
    }
}

impl TurnDecision {
    /// Whether the turn may go on.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed(_))
    }
}

fn pick(found: &[Resource], id: Id, kind: ResourceKind) -> Result<Resource, Error> {
    found
        .iter()
        .find(|resource| resource.id == id && resource.kind == kind)
        .cloned()
        .ok_or(Error::NotFound(id))
}

// ---- Choosing what a thread runs on ------------------------------------------------------------

/// A provider chosen for a thread, with the model and effort it is to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChoice<'a> {
    /// The provider.
    pub id: Id,
    /// The model, as the provider names it.
    pub model: &'a str,
    /// The effort, where one is set.
    pub effort: Option<&'a str>,
    /// The arrangement type the thread is to be bound in, where it has one.
    pub arrangement: Option<Arrangement>,
}

/// Where a thread is to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentChoice {
    /// In an environment that exists: it takes managing it.
    Existing(Id),
    /// In a new one, which is the caller's: it takes what creating one takes.
    New(crate::environments::NewEnvironment),
}

/// What a person chooses a thread to run on, when they create it, fork it or switch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingAsk<'a> {
    /// The node the act is asked of: the topic a thread is made in or forked into, or the thread
    /// that is switched. The work that is to run on the resources is the work there.
    pub node: Id,
    /// What the act needs on that node: `topic.interact` to create or fork there, `thread.manage`
    /// to switch a thread.
    pub permission: Permission,
    /// The provider, with its model and effort.
    pub provider: Option<ProviderChoice<'a>>,
    /// The agent profile.
    pub profile: Option<Id>,
    /// The environment.
    pub environment: Option<EnvironmentChoice>,
    /// The setting `models.allowed` resolved at the node, where any scope sets it.
    pub models_allowed: Option<&'a [String]>,
}

/// What a binding that may be made comes to.
#[derive(Debug, Clone, PartialEq)]
pub struct BindingGrant {
    /// What the person's entries come to on the provider.
    pub provider: Option<TurnGrant>,
    /// What an environment made for the thread is let through by.
    pub created: Option<Box<crate::environments::Created>>,
}

/// Whether a thread may be bound as chosen.
#[derive(Debug, Clone, PartialEq)]
pub enum BindingDecision {
    /// It may.
    Allowed(Box<BindingGrant>),
    /// The caller has what they need, and the choice cannot run for anybody: the provider or the
    /// profile is switched off, or the model, the effort or the arrangement type is not allowed.
    Blocked(Vec<Blocker>),
    /// The caller lacks something: what, thing by thing, and who can give it.
    Refused(Box<Refusal>),
    /// A new environment is beyond what the entries the caller holds allow.
    Exceeds(Vec<crate::environments::Limit>),
    /// The directory chosen is on a machine that is not the caller's.
    NotYourMachine,
}

/// Whether the caller may put a thread on what they chose: the permission the act needs on its node
/// (`topic.interact` to create or fork, `thread.manage` to switch); their own use of the profile and
/// of the provider with the model and effort; and for the environment, `environment.manage` on one
/// that exists or what creating one takes. Nobody's access but the caller's is asked about, and a
/// thread holds nothing by itself. A refusal names everything that is lacking, thing by thing.
pub async fn check_binding(
    conn: &mut Conn,
    principal: &Principal,
    ask: &BindingAsk<'_>,
    policies: &Policies,
) -> Result<BindingDecision, Error> {
    let mut graph = Graph::load(conn, &[ask.node]).await?;
    let node = graph
        .get(ask.node)
        .cloned()
        .ok_or(Error::NotFound(ask.node))?;
    let node_site = graph.site_of(ask.node).ok_or(Error::NotFound(ask.node))?;
    let mut ids: Vec<Id> = Vec::new();
    ids.extend(ask.provider.as_ref().map(|choice| choice.id));
    ids.extend(ask.profile);
    if let Some(EnvironmentChoice::Existing(environment)) = &ask.environment {
        ids.push(*environment);
    }
    let found = read_into(conn, &mut graph, &ids).await?;
    let provider = ask
        .provider
        .as_ref()
        .map(|choice| pick(&found, choice.id, ResourceKind::Provider))
        .transpose()?;
    let profile = ask
        .profile
        .map(|id| pick(&found, id, ResourceKind::Agent))
        .transpose()?;
    let environment = match &ask.environment {
        Some(EnvironmentChoice::Existing(id)) => {
            Some(pick(&found, *id, ResourceKind::Environment)?)
        }
        _ => None,
    };
    let site_for = |resource: &Resource| -> Result<Site, Error> {
        let mut site = site_of(&graph, resource).ok_or(Error::NotFound(resource.id))?;
        site.path = Some(node.path.clone());
        Ok(site)
    };
    let provider_site = provider.as_ref().map(site_for).transpose()?;
    let profile_site = profile.as_ref().map(site_for).transpose()?;
    let environment_site = environment.as_ref().map(site_for).transpose()?;
    let mut sites = vec![node_site.clone()];
    sites.extend(
        [&provider_site, &profile_site, &environment_site]
            .into_iter()
            .flatten()
            .cloned(),
    );
    for site in &sites {
        if let Some(closing) = principal.suspension().closing(&site.orgs) {
            return Err(Error::Suspended { org: closing.org() });
        }
    }
    let home_policy = provider.as_ref().map_or_else(Default::default, |resource| {
        policies.at(resource.home).clone()
    });
    let node_policy = home_policy.node_policy();
    let wanted = [
        ask.permission,
        Permission::ProviderUse,
        Permission::AgentUse,
        Permission::EnvironmentUse,
    ];
    let frame = Frame::load(conn, principal, node_policy, graph, &sites, &wanted).await?;

    let mut lacking: Vec<Lacking> = Vec::new();
    if frame.ways(&node_site, ask.permission).is_empty() {
        lacking.push(Lacking {
            site: node_site.clone(),
            permissions: vec![ask.permission],
            options: None,
            closed: None,
        });
    }
    if let Some(site) = &environment_site
        && frame.ways(site, Permission::EnvironmentUse).is_empty()
    {
        lacking.push(Lacking {
            site: site.clone(),
            permissions: vec![Permission::EnvironmentUse],
            options: None,
            closed: None,
        });
    }
    if let (Some(resource), Some(site)) = (&profile, &profile_site)
        && access(&frame, resource, site, Some(&node.path), false)?.is_err()
    {
        lacking.push(Lacking {
            site: site.clone(),
            permissions: vec![Permission::AgentUse],
            options: None,
            closed: None,
        });
    }
    let mut verdict = ProviderVerdict {
        lacking: Vec::new(),
        blockers: Vec::new(),
        grant: None,
    };
    if let (Some(resource), Some(site), Some(choice)) = (&provider, &provider_site, &ask.provider) {
        let chosen = Chosen {
            model: choice.model,
            effort: choice.effort,
            arrangement: choice.arrangement,
            models_allowed: ask.models_allowed,
        };
        verdict = judge_provider(
            conn,
            &frame,
            &Judging {
                provider: resource,
                site,
                path: &node.path,
                chosen: &chosen,
                kept: home_policy.keeps_to_its_owner(resource),
                state_matters: lacking.is_empty(),
            },
        )
        .await?;
        lacking.append(&mut verdict.lacking);
    }
    let mut blockers = std::mem::take(&mut verdict.blockers);
    if lacking.is_empty() {
        for (resource, stopped) in [(
            &profile,
            (Blocker::ProfileDisabled, Blocker::ProfileDeleted),
        )] {
            if let Some(resource) = resource {
                match resource.unavailable() {
                    Some(Unavailable::Disabled) => blockers.push(stopped.0),
                    Some(Unavailable::Deleted) => blockers.push(stopped.1),
                    None => {}
                }
            }
        }
    }
    // A new environment is the caller's own to make: what that takes is added to the refusal.
    let creation = match &ask.environment {
        Some(EnvironmentChoice::New(new)) => {
            Some(crate::environments::check_creation(conn, principal, new, policies).await?)
        }
        _ => None,
    };
    let mut refusal: Option<Refusal> = None;
    if !lacking.is_empty() {
        if lacking
            .iter()
            .flat_map(|item| item.permissions.iter())
            .any(|permission| !principal.caller().permits(*permission))
        {
            return Ok(BindingDecision::Refused(Box::new(out_of_scope(
                ask.permission,
                ObjectRef {
                    kind: node_site.kind,
                    id: node_site.id,
                },
            ))));
        }
        let graph = frame.graph().clone();
        refusal = Some(
            assemble(
                conn,
                principal,
                node_policy,
                graph,
                &node_site,
                &lacking,
                ask.permission,
            )
            .await?,
        );
    }
    if let Some(crate::environments::Creating::Refused(theirs)) = &creation {
        refusal = Some(match refusal {
            Some(mut ours) => {
                ours.missing.extend(theirs.missing.iter().cloned());
                Refusal::of(ours.required, ours.object, ours.missing)
            }
            None => (**theirs).clone(),
        });
    }
    if let Some(refusal) = refusal {
        return Ok(BindingDecision::Refused(Box::new(refusal)));
    }
    if let Some(blocked) = node_site.archived_node
        && !archive_allows(ask.permission, Act::Use)
    {
        return Err(Error::Archived { node: blocked });
    }
    match creation {
        Some(crate::environments::Creating::Exceeds(limits)) => {
            return Ok(BindingDecision::Exceeds(limits));
        }
        Some(crate::environments::Creating::NotYourMachine) => {
            return Ok(BindingDecision::NotYourMachine);
        }
        _ => {}
    }
    if !blockers.is_empty() {
        return Ok(BindingDecision::Blocked(blockers));
    }
    let created = match creation {
        Some(crate::environments::Creating::Allowed(created)) => Some(created),
        _ => None,
    };
    Ok(BindingDecision::Allowed(Box::new(BindingGrant {
        provider: verdict.grant,
        created,
    })))
}
