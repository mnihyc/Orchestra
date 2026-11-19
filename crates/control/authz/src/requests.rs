//! Access requests: what a person asks for on a thing they lack, the nearest people it is sent to, what a decision gives, and when what was asked for is held.
//!
//! A request asks for permissions of the page of one thing, for the person
//! who asks or for the work in a place, and for the use of a provider it may
//! say which models and efforts. It is [`plan`]ned first: the thing must be
//! there, what is asked must exist on it, must not be held already and must
//! not be closed to everybody (a lock above decides the row; a subscription is
//! kept to its owner), and somebody other than the person who asks must be
//! able to give it. The nearest of them are those it is sent to: the owners,
//! whoever the page names for it, and the admins of the first place above
//! that has any. Whoever could change the page may decide it, and a decision
//! is carried out as that person's ordinary change of the page, so that it
//! gives only what they could have given themself, and exactly what was asked
//! for or less: never another permission, never wider options.
//!
//! A request is for one thing and never for a place in the stead of what is in it: what a person
//! lacks inside a place is asked for on the thing that refused them, one item for each, and a place
//! that is named is asked for the rows of its own page, which open nothing inside it.

use orch_access_contract::decisions::{Closed, Decision, Giver, Givers, ObjectRef, nearest};
use orch_access_contract::pages::{EntryAdd, Holder, PageChange, PageKind, RowChange};
use orch_access_contract::permissions::{
    HolderKind, NamedBy, ObjectKind, Options, Permission, Profile, ProviderUse,
};
use orch_access_contract::roles::PlaceRef;
use orch_db::Conn;
use orch_db::audit::Origin;
use orch_types::{Id, Json, Time};

use crate::engine::{Frame, Graph, Site};
use crate::giving::{self, Acting};
use crate::missing::{self, Who};
use crate::nodes::every_permission;
use crate::pages::{self, ChangeError, Outcome};
use crate::things::{self, Thing};
use crate::usable::ResourcePolicy;
use crate::{Act, Caller, Error, Policy, Principal, documents, nodes, resources};
use std::collections::HashMap;

/// The most characters of the reason that goes with a request.
pub const REASON_MAX_CHARS: usize = 500;

/// For whom the entries of a request would be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whom {
    /// The person who asks.
    Requester,
    /// A selected person; the requester remains the author of the request.
    Person {
        /// The person the resulting entry names.
        id: Id,
    },
    /// The work in a place: threads there may run on the resource, and whoever gives them input
    /// does so on it. It names no person.
    Work {
        /// The place.
        id: Id,
    },
}

impl Whom {
    /// The holder the entries of the request would name.
    #[must_use]
    pub const fn holder(self, requester: Id) -> Holder {
        match self {
            Self::Requester => Holder::User { id: requester },
            Self::Person { id } => Holder::User { id },
            Self::Work { id } => Holder::Node { id },
        }
    }
}

/// The models and efforts a person asks to use a provider with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AskedOptions {
    /// The models.
    pub models: Option<Vec<String>>,
    /// The efforts.
    pub efforts: Option<Vec<String>>,
}

impl AskedOptions {
    /// The options as an entry keeps them: an object with the members that were given.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let mut members = serde_json::Map::new();
        if let Some(models) = &self.models {
            members.insert("models".to_owned(), serde_json::json!(models));
        }
        if let Some(efforts) = &self.efforts {
            members.insert("efforts".to_owned(), serde_json::json!(efforts));
        }
        Json::Object(members)
    }

    fn from_json(value: &Json) -> Option<Self> {
        let words = |member: &str| -> Option<Option<Vec<String>>> {
            match value.get(member) {
                None | Some(Json::Null) => Some(None),
                Some(list) => serde_json::from_value(list.clone()).ok().map(Some),
            }
        };
        Some(Self {
            models: words("models")?,
            efforts: words("efforts")?,
        })
    }
}

/// One thing asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The label shared with a reference, never looked up for an unreadable requester.
    pub label: Option<String>,
    /// The thing, by kind and identifier.
    pub object: ObjectRef,
    /// The permissions of its page.
    pub permissions: Vec<Permission>,
    /// For the use of a provider: the models and efforts.
    pub options: Option<AskedOptions>,
    /// For whom.
    pub whom: Whom,
}

/// What a request asked, as it is kept with the approval that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The label shared when the request was made.
    pub label: Option<String>,
    /// The thing asked about.
    pub object: ObjectRef,
    /// The permissions of the page.
    pub permissions: Vec<Permission>,
    /// For the use of a provider: the models and efforts.
    pub options: Option<AskedOptions>,
    /// For whom.
    pub whom: Whom,
    /// Why.
    pub reason: String,
}

impl Asked {
    /// What the request is as an approval keeps it, in the form the API shows as the request of
    /// an access approval: the permissions, the options, whom the entries would name (`holder`)
    /// and the thread the person was trying to act on (`for_thread`), the last two as the API
    /// writes them, which the caller knows how to write, and the reason. The thing it is about is
    /// kept in the approval's own members.
    #[must_use]
    pub fn to_json(&self, holder: Json, for_thread: Json) -> Json {
        let mut value = serde_json::json!({
            "permissions": self.permissions.iter().map(|one| one.as_str()).collect::<Vec<_>>(),
            "options": self.options.as_ref().map(AskedOptions::to_json),
            "holder": holder,
            "reason": self.reason,
            "for_thread": for_thread,
        });
        if let Some(label) = &self.label {
            value["label"] = serde_json::json!(label);
        }
        value
    }

    /// What an approval keeps of its request, read again; `object` is the thing the approval's own
    /// members name. Nothing where it does not read.
    #[must_use]
    pub fn from_json(object: ObjectRef, value: &Json) -> Option<Self> {
        let holder = value.get("holder")?;
        Some(Self {
            label: value.get("label").and_then(Json::as_str).map(str::to_owned),
            object,
            permissions: value
                .get("permissions")?
                .as_array()?
                .iter()
                .map(|one| one.as_str()?.parse().ok())
                .collect::<Option<Vec<Permission>>>()?,
            options: match value.get("options") {
                None | Some(Json::Null) => None,
                Some(options) => Some(AskedOptions::from_json(options)?),
            },
            whom: match holder.get("kind")?.as_str()? {
                "user" => Whom::Person {
                    id: holder.get("user")?.get("id")?.as_str()?.parse().ok()?,
                },
                "node" => Whom::Work {
                    id: holder.get("node")?.get("id")?.as_str()?.parse().ok()?,
                },
                _ => return None,
            },
            reason: value.get("reason")?.as_str()?.to_owned(),
        })
    }

    /// What the request of `ask` keeps, with the reason that went with it.
    #[must_use]
    pub fn of(ask: &Ask, reason: &str) -> Self {
        Self {
            label: ask.label.clone(),
            object: ask.object,
            permissions: ask.permissions.clone(),
            options: ask.options.clone(),
            whom: ask.whom,
            reason: reason.to_owned(),
        }
    }
}

/// Where an accepted request goes and who it goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// What kind of thing it is.
    pub kind: ObjectKind,
    /// Which.
    pub id: Id,
    /// Its name, for the line that shows the request.
    pub name: String,
    /// Its first owner.
    pub first_owner: Option<Id>,
    /// The id of the node, where the thing is one.
    pub node: Option<Id>,
    /// The node a document lies beside.
    pub beside: Option<Id>,
    /// The root org of the thing, where it has one: the org the event happens in.
    pub root: Option<Id>,
    /// The people it is sent to, and those further up who may decide it too.
    pub givers: Givers,
}

/// Why a request cannot be made as it was asked.
#[derive(Debug, thiserror::Error)]
pub enum AskError {
    /// The access engine failed.
    #[error(transparent)]
    Engine(#[from] Error),
    /// There is no such thing.
    #[error("there is no such thing")]
    NotFound,
    /// A member of the request does not fit.
    #[error("{path}: {message}")]
    Invalid {
        /// The member of the item.
        path: &'static str,
        /// What is wrong.
        message: String,
    },
    /// Nobody can give what is asked, whoever asks.
    #[error("what is asked is closed to everybody")]
    Closed {
        /// Why.
        closed: Closed,
        /// For a row under a lock: the place that set it.
        place: Option<PlaceRef>,
    },
    /// The person holds what they ask for already.
    #[error("what is asked is held already")]
    Held,
    /// Nobody but the person who asks could give it.
    #[error("nobody could give what is asked")]
    Nobody,
}

fn invalid(path: &'static str, message: impl Into<String>) -> AskError {
    AskError::Invalid {
        path,
        message: message.into(),
    }
}

/// The thing a request is about, as the engine finds it: its site, the nodes it lies among and, for
/// a resource, the resource.
async fn locate(
    conn: &mut Conn,
    object: ObjectRef,
) -> Result<(Graph, Site, Option<resources::Resource>), AskError> {
    let thing = things::thing_of(object);
    let found = match thing {
        Thing::Other(other) if !other.kind.is_document() => resources::site(conn, other)
            .await
            .map(|(graph, resource, site)| (graph, site, Some(resource))),
        _ => things::site(conn, thing)
            .await
            .map(|(graph, site)| (graph, site, None)),
    };
    let (graph, site, resource) = match found {
        Ok(found) => found,
        Err(Error::NotFound(_)) => return Err(AskError::NotFound),
        Err(other) => return Err(other.into()),
    };
    let deleted = resource.as_ref().is_some_and(|resource| resource.deleted);
    // A node of another kind than the request says is not the thing it names.
    if deleted || site.kind != object.kind {
        return Err(AskError::NotFound);
    }
    Ok((graph, site, resource))
}

/// Who can give every one of `permissions`: the narrowest of the kinds of people each takes.
fn who_gives(permissions: &[Permission]) -> Who {
    let narrowest = permissions
        .iter()
        .map(|permission| permission.named_by())
        .fold(NamedBy::WhoeverChangesThePage, |so_far, by| {
            match (so_far, by) {
                (NamedBy::Admin, _) | (_, NamedBy::Admin) => NamedBy::Admin,
                (NamedBy::OwnerOrAdmin, _) | (_, NamedBy::OwnerOrAdmin) => NamedBy::OwnerOrAdmin,
                _ => NamedBy::WhoeverChangesThePage,
            }
        });
    match narrowest {
        NamedBy::WhoeverChangesThePage => Who::to_change_the_page(),
        NamedBy::OwnerOrAdmin => Who {
            owners: true,
            named_in: None,
            admins: true,
        },
        NamedBy::Admin => Who {
            owners: false,
            named_in: None,
            admins: true,
        },
    }
}

fn can_decide_rows(frame: &Frame<'_>, site: &Site, permissions: &[Permission]) -> bool {
    let giving = giving::Giving::of(frame, Some(site), PageKind::Live, Acting::Decision);
    permissions
        .iter()
        .all(|permission| giving.can_give(*permission, Profile::Any))
}

/// Page managers are candidates; only those who can give every requested row
/// are recipients. Keep their original distances until the entitled list is split.
async fn entitled_givers(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    permissions: &[Permission],
    requester: Id,
) -> Result<Vec<Giver>, Error> {
    let candidates = missing::givers(conn, frame, site, who_gives(permissions)).await?;
    let asking = Principal::load(conn, Caller::session(requester), frame.now()).await?;
    let outside = site
        .root
        .is_some_and(|root| !asking.memberships().is_member_of(root));
    let mut checked = HashMap::new();
    let mut entitled = Vec::new();
    let everything = every_permission([site.kind]);
    for giver in candidates {
        if giver.user == requester {
            continue;
        }
        // One person can occur as owner, named manager and admin of several
        // ancestors. Ask their current authority once, retaining every distance.
        // An admin decides as one, with As admin on in their own session: the
        // admins of the nearest place that has any are sent a request, so each
        // candidate is asked as they would decide it.
        let allowed = if let Some(allowed) = checked.get(&giver.user) {
            *allowed
        } else {
            let caller = Caller::session(giver.user).with_admin_mode(true);
            let principal = Principal::load(conn, caller, frame.now()).await?;
            let candidate = Frame::load(
                conn,
                &principal,
                frame.policy(),
                frame.graph().clone(),
                std::slice::from_ref(site),
                &everything,
            )
            .await?;
            let allowed = can_decide_rows(&candidate, site, permissions)
                && (!outside || !candidate.ways(site, Permission::AccessExternal).is_empty());
            checked.insert(giver.user, allowed);
            allowed
        };
        if allowed {
            entitled.push(giver);
        }
    }
    Ok(entitled)
}

/// Finds the people who can currently give the requested rows, independently of whether the
/// requester already holds them (a dependency may be asking for its other holders).
pub async fn givers_for(
    conn: &mut Conn,
    requester: &Principal,
    object: ObjectRef,
    permissions: &[Permission],
    options: Option<&orch_access_contract::decisions::MissingOptions>,
    policy: &ResourcePolicy,
) -> Result<Givers, AskError> {
    let (graph, site, resource) = locate(conn, object).await?;
    if permissions.contains(&Permission::ProviderUse)
        && resource
            .as_ref()
            .is_some_and(|resource| policy.keeps_to_its_owner(resource))
    {
        return Ok(Givers::default());
    }
    let frame = Frame::load(
        conn,
        requester,
        policy.node_policy(),
        graph,
        std::slice::from_ref(&site),
        &every_permission([site.kind]),
    )
    .await?;
    if permissions
        .iter()
        .any(|permission| missing::closed_by_lock(&frame, &site, *permission).is_some())
    {
        return Ok(Givers::default());
    }
    let givers = entitled_givers(conn, &frame, &site, permissions, requester.user()).await?;
    let mut entitled = Vec::new();
    for giver in givers {
        if permissions.contains(&Permission::ProviderUse)
            && let Some(resource) = &resource
        {
            // As above: a giver is asked as they would decide.
            let caller = Caller::session(giver.user).with_admin_mode(true);
            let principal = Principal::load(conn, caller, frame.now()).await?;
            let candidate = Frame::load(
                conn,
                &principal,
                frame.policy(),
                frame.graph().clone(),
                std::slice::from_ref(&site),
                &every_permission([site.kind]),
            )
            .await?;
            let Ok(Ok(grant)) = crate::usable::access(&candidate, resource, &site, None, false)
            else {
                continue;
            };
            let held = grant.provider();
            let models = options.and_then(|options| options.models.as_ref());
            let efforts = options.and_then(|options| options.efforts.as_ref());
            let groups = orch_db::queries::resources::groups_of(
                conn,
                &models
                    .into_iter()
                    .flatten()
                    .map(|model| (object.id, model.clone()))
                    .collect::<Vec<_>>(),
            )
            .await
            .map_err(crate::Error::from)?;
            let models_fit = models.map_or(held.models.is_none(), |models| {
                models.iter().all(|model| {
                    crate::options::allows_model(
                        &held,
                        model,
                        groups
                            .get(&(object.id, model.clone()))
                            .map_or(&[][..], Vec::as_slice),
                    )
                })
            });
            let efforts_fit = efforts.map_or(held.efforts.is_none(), |efforts| {
                efforts.iter().all(|effort| held.allows_effort(effort))
            });
            if !models_fit || !efforts_fit {
                continue;
            }
        }
        entitled.push(giver);
    }
    Ok(nearest(&entitled))
}

/// Where a thing stands, as a request about it needs to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Its name.
    pub name: String,
    /// Its root org, where it has one.
    pub root: Option<Id>,
    /// The place whose settings decide about it: the node itself, a resource's home, the node a
    /// document lies beside; none for a resource of the system.
    pub place: Option<Id>,
}

/// Where the thing `object` stands.
pub async fn summary(conn: &mut Conn, object: ObjectRef) -> Result<Summary, AskError> {
    let (_, site, _) = locate(conn, object).await?;
    let place = match things::thing_of(object) {
        Thing::Node(id) => Some(id),
        Thing::Other(other) if other.kind.is_document() => site.beside,
        Thing::Other(_) => site.role_place,
    };
    Ok(Summary {
        name: site.name.clone(),
        root: site.root,
        place,
    })
}

/// Whether `person` would pass the check for changing the page of `object`, which is what makes
/// somebody able to decide a request about it; and if not, what they lack and who can give it.
pub async fn may_decide(
    conn: &mut Conn,
    person: &Principal,
    object: ObjectRef,
    policy: Policy,
) -> Result<Decision, Error> {
    match things::thing_of(object) {
        Thing::Node(id) => {
            nodes::check(conn, person, id, Permission::AccessManage, Act::Use, policy).await
        }
        Thing::Other(other) if other.kind.is_document() => {
            documents::check(conn, person, other.id, Permission::AccessManage, policy).await
        }
        Thing::Other(other) => {
            resources::check(conn, person, other, Permission::AccessManage, policy).await
        }
    }
}

/// Checks a request for one thing and finds who it is sent to: `requester` is the person who
/// asks, and `policy` what the settings say at the thing's home.
pub async fn plan(
    conn: &mut Conn,
    requester: &Principal,
    ask: &Ask,
    policy: &ResourcePolicy,
) -> Result<Sent, AskError> {
    if ask.permissions.is_empty() {
        return Err(invalid("permissions", "must name at least one permission"));
    }
    let (graph, site, resource) = locate(conn, ask.object).await?;
    for permission in &ask.permissions {
        if *permission == Permission::AccessOwn || !permission.exists_on(site.kind) {
            return Err(invalid(
                "permissions",
                format!(
                    "{} is no permission that can be asked for on a {}",
                    permission.as_str(),
                    site.kind
                ),
            ));
        }
    }
    if ask.options.is_some() && !ask.permissions.contains(&Permission::ProviderUse) {
        return Err(invalid(
            "options",
            "models and efforts are asked for with the use of a provider",
        ));
    }
    let node_policy = policy.node_policy();
    if let Whom::Work { id: place } = ask.whom {
        let fits = ask
            .permissions
            .iter()
            .all(|permission| HolderKind::Node.fits(*permission, site.kind, site.of_system));
        if !fits {
            return Err(invalid(
                "holder",
                "the work in a place is named in the Use row of a resource, and in no other",
            ));
        }
        let may = matches!(
            nodes::check(
                conn,
                requester,
                place,
                Permission::AccessManage,
                Act::Use,
                node_policy
            )
            .await,
            Ok(Decision::Allowed { .. })
        );
        if !may {
            return Err(invalid(
                "holder",
                "the work in a place is named by whoever may change the page of that place",
            ));
        }
    }
    let everything = every_permission([site.kind]);
    let frame = Frame::load(
        conn,
        requester,
        node_policy,
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    for permission in &ask.permissions {
        if let Some(place) = missing::closed_by_lock(&frame, &site, *permission) {
            return Err(AskError::Closed {
                closed: Closed::Locked,
                place: Some(place),
            });
        }
    }
    if ask.permissions.contains(&Permission::ProviderUse)
        && resource
            .as_ref()
            .is_some_and(|resource| policy.keeps_to_its_owner(resource))
    {
        return Err(AskError::Closed {
            closed: Closed::SubscriptionNotShared,
            place: None,
        });
    }
    if holds_for(conn, &frame, &site, ask.whom, &ask.permissions).await? {
        return Err(AskError::Held);
    }
    let givers = entitled_givers(conn, &frame, &site, &ask.permissions, requester.user()).await?;
    let givers = nearest(&givers);
    if givers.nearest.is_empty() {
        return Err(AskError::Nobody);
    }
    Ok(Sent {
        kind: site.kind,
        id: site.id,
        name: site.name.clone(),
        first_owner: site.first_owner,
        node: site.kind.node().map(|_| site.id),
        beside: site.beside,
        root: site.root,
        givers,
    })
}

/// Whether what is asked is held: by the person themself, or by the work in the place the entries
/// would name.
fn holds_all(frame: &Frame<'_>, site: &Site, whom: Whom, permissions: &[Permission]) -> bool {
    permissions.iter().all(|permission| match whom {
        Whom::Requester | Whom::Person { .. } => !frame.ways(site, *permission).is_empty(),
        Whom::Work { id } => frame
            .chain(site, *permission)
            .holders(frame.now())
            .iter()
            .any(|named| named.entry.holder == Holder::Node { id }),
    })
}

async fn holds_for(
    conn: &mut Conn,
    frame: &Frame<'_>,
    site: &Site,
    whom: Whom,
    permissions: &[Permission],
) -> Result<bool, Error> {
    if let Whom::Person { id } = whom {
        let recipient = Principal::load(conn, Caller::session(id), frame.now()).await?;
        let target = Frame::load(
            conn,
            &recipient,
            frame.policy(),
            frame.graph().clone(),
            std::slice::from_ref(site),
            &every_permission([site.kind]),
        )
        .await?;
        return Ok(holds_all(&target, site, whom, permissions));
    }
    Ok(holds_all(frame, site, whom, permissions))
}

/// Whether what was asked is held now, however it came about: the person holds every permission,
/// or the page names the work in the place for each.
pub async fn is_held(
    conn: &mut Conn,
    requester: &Principal,
    object: ObjectRef,
    asked: &Asked,
    policy: Policy,
) -> Result<bool, Error> {
    let thing = things::thing_of(object);
    let (graph, site) = match things::site(conn, thing).await {
        Ok(found) => found,
        Err(Error::NotFound(_)) => return Ok(false),
        Err(other) => return Err(other),
    };
    let everything = every_permission([site.kind]);
    let frame = Frame::load(
        conn,
        requester,
        policy,
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    holds_for(conn, &frame, &site, asked.whom, &asked.permissions).await
}

/// How a person stands to deciding a request now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Standing {
    /// They would pass the check for changing the page of the thing, and so may decide.
    pub may_decide: bool,
    /// They hold what the request asks for, so that approving it is not refused as giving more
    /// than they have: [`giving::Giving::judge`] of a decision, asked beforehand.
    pub can_give: bool,
    /// The request is sent to them: they are among the nearest who could give what it asks.
    pub sent_to_them: bool,
}

/// How `person` stands to deciding a request for `permissions` of the page of `object` made by
/// `requester`: whether they may decide it, and whether it was sent to them.
pub async fn standing(
    conn: &mut Conn,
    person: &Principal,
    object: ObjectRef,
    permissions: &[Permission],
    requester: Id,
    policy: Policy,
) -> Result<Standing, Error> {
    if person.user() == requester {
        return Ok(Standing::default());
    }
    let thing = things::thing_of(object);
    let (graph, site) = match things::site(conn, thing).await {
        Ok(found) => found,
        Err(Error::NotFound(_)) => return Ok(Standing::default()),
        Err(other) => return Err(other),
    };
    let everything = every_permission([site.kind]);
    let frame = Frame::load(
        conn,
        person,
        policy,
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    let may_decide = !frame.ways(&site, Permission::AccessManage).is_empty();
    if !may_decide {
        return Ok(Standing::default());
    }
    let can_give = can_decide_rows(&frame, &site, permissions);
    let givers = entitled_givers(conn, &frame, &site, permissions, requester).await?;
    let split = nearest(&givers);
    Ok(Standing {
        may_decide,
        can_give,
        sent_to_them: split.nearest.contains(&person.user()),
    })
}

/// The name of `object`, for a person who may know it: one who holds anything on it. A request
/// names a thing only to those who can already see it, whom it concerns.
pub async fn named_for(
    conn: &mut Conn,
    person: &Principal,
    object: ObjectRef,
    policy: Policy,
) -> Result<Option<String>, Error> {
    let thing = things::thing_of(object);
    let (graph, site) = match things::site(conn, thing).await {
        Ok(found) => found,
        Err(Error::NotFound(_)) => return Ok(None),
        Err(other) => return Err(other),
    };
    let everything = every_permission([site.kind]);
    let frame = Frame::load(
        conn,
        person,
        policy,
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    Ok(frame.holds_any(&site).then(|| site.name.clone()))
}

/// What an approver decides on a request that they approve.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Giving {
    /// The permissions to give, if fewer than were asked.
    pub permissions: Option<Vec<Permission>>,
    /// Narrower options, as written on an entry.
    pub options: Option<Json>,
    /// When what is given ends.
    pub expires_at: Option<Time>,
}

/// Why a request could not be approved as decided.
#[derive(Debug, thiserror::Error)]
pub enum GrantError {
    /// The access engine failed.
    #[error(transparent)]
    Engine(#[from] Error),
    /// The decision does not fit the request.
    #[error("{path}: {message}")]
    Invalid {
        /// The member of the decision.
        path: &'static str,
        /// What is wrong.
        message: String,
    },
    /// The change of the page that the decision comes to was refused, as it would be if the
    /// approver made it themself.
    #[error("the change of the page was refused")]
    Change(Box<ChangeError>),
    /// Nobody decides their own request.
    #[error("a request is not decided by the person who made it")]
    OwnRequest,
}

impl From<ChangeError> for GrantError {
    fn from(error: ChangeError) -> Self {
        Self::Change(Box::new(error))
    }
}

fn unfit(path: &'static str, message: impl Into<String>) -> GrantError {
    GrantError::Invalid {
        path,
        message: message.into(),
    }
}

/// The options an entry is written with: those the approver chose, which stay within what was
/// asked, or else what was asked.
fn options_given(asked: &Asked, chosen: Option<&Json>) -> Result<Json, GrantError> {
    let Some(chosen) = chosen else {
        return Ok(asked.options.as_ref().map_or_else(
            || Json::Object(serde_json::Map::new()),
            AskedOptions::to_json,
        ));
    };
    let parsed = Options::parse(Permission::ProviderUse, chosen).map_err(|faults| {
        unfit(
            "options",
            faults
                .first()
                .map_or_else(|| "do not read".to_owned(), |fault| fault.message.clone()),
        )
    })?;
    let Options::ProviderUse(ProviderUse {
        models,
        efforts,
        arrangements,
    }) = &parsed
    else {
        return Err(unfit("options", "are for the use of a provider"));
    };
    let within = |given: &Option<Vec<String>>, wanted: Option<&Vec<String>>, member: &str| match (
        given, wanted,
    ) {
        (Some(given), Some(wanted)) if given.iter().any(|one| !wanted.contains(one)) => Err(unfit(
            "options",
            format!("{member} reach beyond what was asked for"),
        )),
        _ => Ok(()),
    };
    within(
        models,
        asked.options.as_ref().and_then(|o| o.models.as_ref()),
        "models",
    )?;
    within(
        efforts,
        asked.options.as_ref().and_then(|o| o.efforts.as_ref()),
        "efforts",
    )?;
    // What the approver left out stays as asked: an option that was asked for is not dropped.
    let mut written = parsed.to_json();
    if let (Json::Object(members), Some(wanted)) = (&mut written, &asked.options) {
        if models.is_none()
            && let Some(models) = &wanted.models
        {
            members.insert("models".to_owned(), serde_json::json!(models));
        }
        if efforts.is_none()
            && let Some(efforts) = &wanted.efforts
        {
            members.insert("efforts".to_owned(), serde_json::json!(efforts));
        }
    }
    let _ = arrangements;
    Ok(written)
}

/// Carries out an approval: names the holder the request asked for in the rows of the permissions
/// it asked for, or of fewer, with the options it asked for or narrower ones and an end the
/// approver may set. It is `approver`'s ordinary change of the page of the thing, so that they must
/// hold the right to make it and to give each permission, a row under a lock above is refused, and
/// somebody from outside the root org is named only as the approver may name them. The entries
/// are marked as written by an approved request.
#[allow(
    clippy::too_many_arguments,
    reason = "the approver, the request and its thing, what was decided, and where it is done"
)]
pub async fn grant(
    conn: &mut Conn,
    approver: &Principal,
    requester: Id,
    object: ObjectRef,
    asked: &Asked,
    giving: &Giving,
    policy: &ResourcePolicy,
    origin: &Origin,
) -> Result<(Outcome, Vec<Permission>), GrantError> {
    if approver.user() == requester {
        return Err(GrantError::OwnRequest);
    }
    let permissions: Vec<Permission> = match &giving.permissions {
        None => asked.permissions.clone(),
        Some(chosen) => {
            if chosen.is_empty() {
                return Err(unfit(
                    "permissions",
                    "must name at least one: to give nothing, deny",
                ));
            }
            if let Some(beyond) = chosen.iter().find(|one| !asked.permissions.contains(one)) {
                return Err(unfit(
                    "permissions",
                    format!("{} was not asked for", beyond.as_str()),
                ));
            }
            chosen.clone()
        }
    };
    if giving.options.is_some() && !permissions.contains(&Permission::ProviderUse) {
        return Err(unfit("options", "are for the use of a provider"));
    }
    if let Some(end) = giving.expires_at
        && end <= approver.now()
    {
        return Err(unfit("expires_at", "must be in the future"));
    }
    let holder = asked.whom.holder(requester);
    let mut rows: Vec<RowChange> = Vec::with_capacity(permissions.len());
    for permission in &permissions {
        let options = if *permission == Permission::ProviderUse {
            Some(options_given(asked, giving.options.as_ref())?)
        } else {
            None
        };
        rows.push(RowChange {
            permission: permission.as_str().to_owned(),
            profile: None,
            inherit: None,
            locked: None,
            add: Some(vec![EntryAdd {
                holder,
                options,
                expires_at: giving.expires_at,
                note: None,
            }]),
            update: None,
            remove: None,
        });
    }
    let change = PageChange {
        rows: Some(rows),
        ..PageChange::default()
    };
    let target = things::thing_of(object).target();
    let outcome =
        pages::change_for_request(conn, approver, target, &change, policy, origin).await?;
    Ok((outcome, permissions))
}
