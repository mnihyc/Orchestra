//! Private sender provenance for explicit notices, shared by HTTP and standalone delivery.
//! An opaque operation reference in the notice is not authority: only its immutable private
//! record, current original credentials and the actual current native subject can permit copying.

use chrono::SubsecRound;
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{NodePermission, ObjectKind, Permission, PermissionName};
use orch_api_contract::notification_sources::{self as sources, Source};
use orch_api_contract::notifications::{ChatPersonGrant, PersonNotifyCreate};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::routes::{Endpoint, Route};
use orch_authz::key_targets::{self, TargetError};
use orch_authz::{Act, Caller, CapturedOperation, Policy, Principal};
use orch_db::queries::notifications::operations;
use orch_db::queries::notifications::outbox::Snapshot;
use orch_db::queries::notifications::people::{Announced, NewNotification};
use orch_db::queries::{chats, transaction_guard};
use orch_db::{Conn, DbError};
use orch_operation_scope::{BindingError, sources::Facts};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Current native subject identity; placement always comes from the real stored object.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "subject", deny_unknown_fields)]
pub enum Subject {
    /// A node, document or resource with its own native page.
    Object(ObjectRef),
    /// A native chat, independently of the node or org that keeps it.
    Chat(Id),
}

/// The actual native act producing this notice; never a nearby operation alias.
#[derive(Clone, Copy)]
pub enum Operation {
    /// Explicitly notify current readers of one subject.
    Person,
    /// The notifying half of one atomic native Chat person grant.
    ChatGrant,
}
impl Operation {
    fn of(operation: &str) -> Option<Self> {
        match operation {
            "PersonNotifyCreate" => Some(Self::Person),
            "ChatPersonGrant" => Some(Self::ChatGrant),
            _ => None,
        }
    }
    /// The native producer's canonical source row.
    pub fn declaration(self) -> &'static Declaration<Source> {
        match self {
            Self::Person => &sources::PERSON_CREATE,
            Self::ChatGrant => &sources::CHAT_GRANT,
        }
    }
    /// The actual canonical route, including its ordinary legacy eligibility.
    pub fn route(self) -> Route {
        match self {
            Self::Person => PersonNotifyCreate::ROUTE,
            Self::ChatGrant => ChatPersonGrant::ROUTE,
        }
    }
}

/// Source preparation failed; a closed source is returned separately from a database failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A current query failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The ordinary current source gate failed.
    #[error(transparent)]
    Access(#[from] orch_authz::Error),
    /// Current account setup policy could not be evaluated.
    #[error(transparent)]
    Account(#[from] orch_settings::account_policy::Error),
    /// Canonical source metadata was inconsistent.
    #[error(transparent)]
    Binding(#[from] BindingError),
    /// Private evidence cannot identify the actual accepted native notice.
    #[error("the explicit notice producer is unavailable")]
    Refused,
    /// Private stored data could not be encoded.
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
}

/// The same ordinary readable-source door as the explicit-notice producer.
pub async fn reads(
    conn: &mut Conn,
    principal: &Principal,
    subject: Subject,
) -> Result<bool, orch_authz::Error> {
    match subject {
        Subject::Object(object) if object.kind.node().is_some() => Ok(!orch_authz::nodes::holding(
            conn,
            principal,
            &[object.id],
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await?
        .is_empty()),
        Subject::Object(object) if object.kind.is_document() => match orch_authz::documents::check(
            conn,
            principal,
            object.id,
            Permission::DocView,
            Policy::default(),
        )
        .await
        {
            Ok(Decision::Allowed { .. }) => Ok(true),
            Ok(Decision::Refused(_))
            | Err(
                orch_authz::Error::NotFound(_)
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::Suspended { .. },
            ) => Ok(false),
            Err(error) => Err(error),
        },
        Subject::Object(object) => match orch_authz::things::effective(
            conn,
            principal,
            orch_authz::things::Thing::Other(object),
            Policy::default(),
        )
        .await
        {
            Ok(held) => Ok(!held.held.is_empty()),
            Err(
                orch_authz::Error::NotFound(_)
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::Suspended { .. },
            ) => Ok(false),
            Err(error) => Err(error),
        },
        Subject::Chat(id) => {
            let Some(row) = chats::chat(conn, id).await? else {
                return Ok(false);
            };
            Ok(orch_authz::chats::listed_access(conn, principal, &[row])
                .await?
                .contains_key(&id))
        }
    }
}

/// Resolve the native producer's complete factual role, without granting source visibility.
pub async fn facts(conn: &mut Conn, subject: Subject) -> Result<Vec<Facts<Source>>, TargetError> {
    let target = match subject {
        Subject::Object(object) => key_targets::page_object(conn, object).await?,
        Subject::Chat(id) => key_targets::chat(conn, id).await?,
    };
    let family = target.object.kind;
    if !sources::SUBJECT_KINDS.contains(&family) {
        return Err(TargetError::Missing);
    }
    Ok(vec![Facts::one(Source::Subject, family, target)])
}

/// The same factual subject plus the actual optional copy selection of the native act.
pub async fn operation_facts(
    conn: &mut Conn,
    operation: Operation,
    subject: Subject,
    copy: bool,
) -> Result<Vec<Facts<Source>>, TargetError> {
    if matches!(operation, Operation::ChatGrant) && !matches!(subject, Subject::Chat(_)) {
        return Err(TargetError::Missing);
    }
    let mut facts = facts(conn, subject).await?;
    if matches!(operation, Operation::ChatGrant) {
        let selected = if copy {
            facts[0].groups.clone()
        } else {
            Vec::new()
        };
        facts.push(Facts::collection(Source::NoticeChats, selected));
    }
    Ok(facts)
}

async fn current(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    subject: Subject,
) -> Result<Option<Principal>, Error> {
    if !transaction_guard::in_transaction(conn) {
        return Err(Error::Refused);
    }
    let Some(operation) = Operation::of(origin.operation()) else {
        return Ok(None);
    };
    transaction_guard::lock_action(conn).await?;
    let Some(user) = transaction_guard::lock_native_actor(conn, origin.user()).await? else {
        return Ok(None);
    };
    let caller = match origin.restore_current(conn, clock).await {
        Ok(Some(caller)) => caller,
        Ok(None) | Err(orch_authz::Error::Stored(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if caller.user() != user.id
        || caller
            .grant()
            .is_some_and(|grant| grant.token_ids().next().is_none())
    {
        return Ok(None);
    }
    let facts = match operation_facts(conn, operation, subject, true).await {
        Ok(facts) => facts,
        Err(TargetError::Missing | TargetError::Authz(orch_authz::Error::NotFound(_))) => {
            return Ok(None);
        }
        Err(TargetError::Db(error)) => return Err(error.into()),
        Err(TargetError::Authz(error)) => return Err(error.into()),
    };
    let plan = orch_operation_scope::sources::bind_required(
        operation.route(),
        operation.declaration(),
        &facts,
    )?;
    let caller = match plan.admit(&caller, false) {
        Ok(caller) => caller,
        Err(BindingError::Refused) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut principal = match Principal::load(conn, caller, Time::new(clock.now())).await {
        Ok(principal) => principal,
        Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    if user.kind != "service" && !orch_settings::account_policy::configured(&user) {
        let requirement = orch_settings::account_policy::requirement(conn, &user).await?;
        if !requirement.orgs.is_empty() {
            let held: BTreeSet<_> = requirement.orgs.iter().copied().collect();
            if plan.required_facets()?.iter().any(|facet| {
                facet.target.node_ancestors.iter().any(|node| {
                    node.kind == KeyTargetKind::Object(ObjectKind::Org)
                        && node.id.is_some_and(|id| held.contains(&id))
                })
            }) {
                return Ok(None);
            }
            principal = principal.held_for_second_step(orch_authz::suspension::SecondStepHold {
                orgs: requirement.orgs,
                setup: true,
                sources: requirement.sources,
                methods: Vec::new(),
            });
        }
    }
    if !reads(conn, &principal, subject).await? || !principal.caller().key_deadline_allows(clock) {
        return Ok(None);
    }
    Ok(Some(principal))
}

#[derive(Serialize, Deserialize, PartialEq)]
enum Producer {
    #[serde(rename = "explicit_person_notice")]
    PersonNotice,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Evidence {
    producer: Producer,
    version: u8,
    origin: CapturedOperation,
    subject: Subject,
    expected: Snapshot,
    receivers: Vec<(Id, Id)>,
}

/// A real accepted producer, kept private until all actual Inbox identities are known.
pub struct Capture {
    id: Id,
    origin: CapturedOperation,
    subject: Subject,
    expected: Option<Snapshot>,
    receivers: Vec<(Id, Id)>,
}

impl Capture {
    /// Capture the complete actual operation; the caller's current original credentials,
    /// current typed subject and ordinary source Read are checked on this same transaction.
    pub async fn prepare(
        conn: &mut Conn,
        clock: &dyn Clock,
        accepted: &Caller,
        subject: Subject,
        operation: Operation,
        id: Id,
    ) -> Result<Self, Error> {
        // Ordinary/legacy HTTP callers keep their native cold path. The same actual
        // typed source still supplies a complete canonical origin before deferred IO.
        let facts = match operation_facts(conn, operation, subject, true).await {
            Ok(facts) => facts,
            Err(TargetError::Missing) => return Err(Error::Refused),
            Err(TargetError::Db(error)) => return Err(error.into()),
            Err(TargetError::Authz(error)) => return Err(error.into()),
        };
        let plan = orch_operation_scope::sources::bind_required(
            operation.route(),
            operation.declaration(),
            &facts,
        )?;
        let accepted = plan.admit(accepted, false)?;
        let origin = CapturedOperation::capture(&accepted).map_err(|_| Error::Refused)?;
        let principal = current(conn, clock, &origin, subject)
            .await?
            .ok_or(Error::Refused)?;
        Ok(Self {
            id,
            origin: CapturedOperation::capture(principal.caller()).map_err(|_| Error::Refused)?,
            subject,
            expected: None,
            receivers: Vec::new(),
        })
    }

    /// Link only genuine rows returned by the one native Inbox/fanout writer.
    pub fn record(
        &mut self,
        notice: &NewNotification<'_>,
        actual: &Announced,
    ) -> Result<(), Error> {
        if notice.kind != "person.notify"
            || notice.data.get("operation_id") != Some(&serde_json::json!(self.id))
            || notice.data.get("by") != Some(&serde_json::json!(self.origin.user()))
            || !self.subject.matches_notice(notice)
            || self
                .receivers
                .iter()
                .any(|(user, id)| *user == notice.user_id || *id == actual.id)
        {
            return Err(Error::Refused);
        }
        let snapshot = normalized(&Snapshot::person_notice(None, notice)).ok_or(Error::Refused)?;
        if self
            .expected
            .as_ref()
            .is_some_and(|expected| *expected != snapshot)
        {
            return Err(Error::Refused);
        }
        self.expected = Some(snapshot);
        self.receivers.push((notice.user_id, actual.id));
        Ok(())
    }

    /// Persist no credential information in the public notice. Its opaque reference names
    /// this immutable private accepted choice and actual recipient/Inbox associations.
    pub async fn save(self, conn: &mut Conn) -> Result<(), Error> {
        if let Some(expected) = self.expected {
            let at = expected.at.get();
            let evidence = Evidence {
                producer: Producer::PersonNotice,
                version: 1,
                origin: self.origin,
                subject: self.subject,
                expected,
                receivers: self.receivers,
            };
            operations::save(conn, self.id, &serde_json::to_value(evidence)?, at).await?;
        }
        Ok(())
    }
}

impl Subject {
    fn matches_notice(self, notice: &NewNotification<'_>) -> bool {
        match self {
            Self::Chat(id) => {
                notice.chat_id == Some(id)
                    && notice.data.get("chat_id") == Some(&serde_json::json!(id))
            }
            Self::Object(ObjectRef {
                kind: ObjectKind::Document,
                id,
            }) => notice.data.get("document_id") == Some(&serde_json::json!(id)),
            Self::Object(object) if object.kind.node().is_some() => {
                notice.node_id == Some(object.id)
                    && notice.data.get("target_node_id") == Some(&serde_json::json!(object.id))
            }
            Self::Object(object) => {
                notice.data.get("resource_id") == Some(&serde_json::json!(object.id))
                    && notice.data.get("resource_kind").and_then(Json::as_str)
                        == Some(object.kind.as_str())
            }
        }
    }
}

/// The original immutable source of a native rendered message; never reusable admission.
pub(crate) struct Opened {
    id: Id,
    evidence: Arc<Evidence>,
    inbox: Id,
    receiver: Id,
    placement: KeyResolvedTarget,
    names: Vec<ObjectRef>,
    places: Vec<orch_db::queries::notifications::people::PlaceRow>,
    org_name: Option<String>,
}

pub(crate) enum Loaded {
    Legacy,
    Closed,
    Source(Box<Opened>),
}

pub(crate) async fn load(
    conn: &mut Conn,
    clock: &dyn Clock,
    receiver: Id,
    snapshot: &Snapshot,
) -> Result<Loaded, Error> {
    if snapshot.data.get("operation_id").is_none() {
        return Ok(Loaded::Legacy);
    }
    let Some(id) = snapshot
        .data
        .get("operation_id")
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Ok(if snapshot.kind == "person.notify" {
            Loaded::Closed
        } else {
            Loaded::Legacy
        });
    };
    let Some(json) = operations::get(conn, id).await? else {
        return Ok(if snapshot.kind == "person.notify" {
            Loaded::Closed
        } else {
            Loaded::Legacy
        });
    };
    if snapshot.kind != "person.notify"
        && json.get("producer").and_then(Json::as_str) != Some("explicit_person_notice")
    {
        return Ok(Loaded::Legacy);
    }
    let Ok(evidence) = serde_json::from_value::<Evidence>(json) else {
        return Ok(Loaded::Closed);
    };
    let Some(expected) = normalized(snapshot) else {
        return Ok(Loaded::Closed);
    };
    if evidence.version != 1
        || evidence.expected != expected
        || !snapshot
            .id
            .is_some_and(|inbox| evidence.receivers.contains(&(receiver, inbox)))
    {
        return Ok(Loaded::Closed);
    }
    let facts = match facts(conn, evidence.subject).await {
        Ok(facts) => facts,
        Err(TargetError::Missing | TargetError::Authz(orch_authz::Error::NotFound(_))) => {
            return Ok(Loaded::Closed);
        }
        Err(TargetError::Db(error)) => return Err(error.into()),
        Err(TargetError::Authz(error)) => return Err(error.into()),
    };
    let placement = facts[0].groups[0].1[0].clone();
    if placement.node_ancestors.last().and_then(|node| node.id) != snapshot.node_id {
        return Ok(Loaded::Closed);
    }
    let mut opened = Opened {
        id,
        evidence: Arc::new(evidence),
        inbox: snapshot.id.ok_or(Error::Refused)?,
        receiver,
        placement,
        names: Vec::new(),
        places: Vec::new(),
        org_name: None,
    };
    if !opened.project_names(conn, clock).await? || opened.check(conn, clock).await?.is_none() {
        return Ok(Loaded::Closed);
    }
    Ok(Loaded::Source(Box::new(opened)))
}

fn normalized(snapshot: &Snapshot) -> Option<Snapshot> {
    let mut actual = snapshot.clone();
    actual.id = None;
    // PostgreSQL persists this native event time at microsecond precision. Initial
    // fanout may still hold the producer's finer clock value; no key deadline is rounded.
    actual.at = Time::new(actual.at.get().trunc_subsecs(6));
    let data =
        orch_db::queries::notifications::people::notification_data(&snapshot.kind, &snapshot.data);
    // Initial fanout and a later enabled route use the same native data; the Inbox
    // producer adds its action vocabulary envelope. A present envelope must be exact.
    if snapshot
        .data
        .get(orch_db::queries::notifications::people::NOTIFICATION_DETAILS_MEMBER)
        .is_some()
        && snapshot.data != data
    {
        return None;
    }
    actual.data = data;
    Some(actual)
}

impl Opened {
    pub(crate) fn inbox(&self) -> Id {
        self.inbox
    }
    pub(crate) fn places(&self) -> &[orch_db::queries::notifications::people::PlaceRow] {
        &self.places
    }
    pub(crate) fn org_name(&self) -> Option<&str> {
        self.org_name.as_deref()
    }

    async fn project_names(&mut self, conn: &mut Conn, clock: &dyn Clock) -> Result<bool, Error> {
        let Some(principal) =
            current(conn, clock, &self.evidence.origin, self.evidence.subject).await?
        else {
            return Ok(false);
        };
        let ids: Vec<_> = self
            .placement
            .node_ancestors
            .iter()
            .filter_map(|node| node.id)
            .collect();
        let graph = orch_authz::engine::Graph::load(conn, &ids).await?;
        for id in &ids {
            let Some(node) = graph.get(*id) else {
                return Ok(false);
            };
            let object = ObjectRef {
                kind: node.kind.into(),
                id: *id,
            };
            if named_principal(conn, &principal, self.evidence.subject, &[object])
                .await?
                .is_some()
            {
                self.names.push(object);
            }
        }
        self.org_name = ids
            .first()
            .filter(|id| self.names.iter().any(|source| source.id == **id))
            .and_then(|id| graph.get(*id))
            .map(|node| node.name.clone());
        if let Some(home) = ids
            .last()
            .filter(|id| self.names.iter().any(|source| source.id == **id))
        {
            let Some(node) = graph.get(*home) else {
                return Ok(false);
            };
            self.places
                .push(orch_db::queries::notifications::people::PlaceRow {
                    id: *home,
                    kind: node.kind.as_str().to_owned(),
                    name: node.name.clone(),
                    path: ids
                        .iter()
                        .filter(|id| self.names.iter().any(|source| source.id == **id))
                        .filter_map(|id| graph.get(*id))
                        .map(|node| node.name.clone())
                        .collect(),
                    org_id: node.root,
                    org_name: self.org_name.clone().unwrap_or_default(),
                });
        }
        Ok(principal.caller().key_deadline_allows(clock))
    }

    /// Repeat the exact original operation, actual typed source and ordinary source door
    /// before reservation and before each further send; no sender is reconstructed by id.
    pub(crate) async fn check(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Caller>, Error> {
        let Some(json) = operations::get(conn, self.id).await? else {
            return Ok(None);
        };
        let Ok(evidence) = serde_json::from_value::<Evidence>(json) else {
            return Ok(None);
        };
        if evidence != *self.evidence {
            return Ok(None);
        }
        let Some(principal) =
            current(conn, clock, &self.evidence.origin, self.evidence.subject).await?
        else {
            return Ok(None);
        };
        let facts = match facts(conn, self.evidence.subject).await {
            Ok(facts) => facts,
            Err(TargetError::Missing | TargetError::Authz(orch_authz::Error::NotFound(_))) => {
                return Ok(None);
            }
            Err(TargetError::Db(error)) => return Err(error.into()),
            Err(TargetError::Authz(error)) => return Err(error.into()),
        };
        if facts[0].groups[0].1[0] != self.placement {
            return Ok(None);
        }
        // This is the same observational recipient door as native outbox selection;
        // it supplies no sender authority and never replaces the restored producer.
        let recipient =
            match Principal::load(conn, Caller::session(self.receiver), clock.time()).await {
                Ok(recipient) => recipient,
                Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
        if !reads(conn, &recipient, self.evidence.subject).await? {
            return Ok(None);
        }
        let principal = if self.names.is_empty() {
            principal
        } else {
            let Some(principal) =
                named_principal(conn, &principal, self.evidence.subject, &self.names).await?
            else {
                return Ok(None);
            };
            principal
        };
        Ok(principal
            .caller()
            .key_deadline_allows(clock)
            .then(|| principal.caller().clone()))
    }
}

async fn named_principal(
    conn: &mut Conn,
    principal: &Principal,
    subject: Subject,
    names: &[ObjectRef],
) -> Result<Option<Principal>, Error> {
    let Some(operation) = principal
        .caller()
        .admitted_operation()
        .and_then(Operation::of)
    else {
        return Ok(None);
    };
    let facts = match operation_facts(conn, operation, subject, true).await {
        Ok(facts) => facts,
        Err(TargetError::Missing | TargetError::Authz(orch_authz::Error::NotFound(_))) => {
            return Ok(None);
        }
        Err(TargetError::Db(error)) => return Err(error.into()),
        Err(TargetError::Authz(error)) => return Err(error.into()),
    };
    let mut plan = orch_operation_scope::sources::bind_required(
        operation.route(),
        operation.declaration(),
        &facts,
    )?;
    let binding = operation
        .declaration()
        .bindings
        .iter()
        .find(|binding| binding.source == Source::NoticePlaceNames)
        .ok_or(Error::Refused)?;
    let indices = orch_operation_scope::sources::indices(&plan, binding)?;
    let mut groups = std::collections::BTreeMap::<ObjectKind, Vec<KeyResolvedTarget>>::new();
    for object in names {
        let target = match key_targets::node(conn, object.id).await {
            Ok(target) => target,
            Err(TargetError::Missing | TargetError::Authz(orch_authz::Error::NotFound(_))) => {
                return Ok(None);
            }
            Err(TargetError::Db(error)) => return Err(error.into()),
            Err(TargetError::Authz(error)) => return Err(error.into()),
        };
        if target.object.kind != KeyTargetKind::Object(object.kind) {
            return Ok(None);
        }
        groups.entry(object.kind).or_default().push(target);
    }
    let groups = groups
        .into_iter()
        .map(|(kind, targets)| (KeyTargetKind::Object(kind), targets))
        .collect::<Vec<_>>();
    for index in &indices {
        plan.bind_collection(*index, &groups)?;
    }
    let caller = match plan.admit_response_sources(principal.caller(), &indices) {
        Ok(caller) => caller,
        Err(BindingError::Refused) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut current = match Principal::load(conn, caller, principal.now()).await {
        Ok(current) => current,
        Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    if let Some(hold) = principal.second_step_hold() {
        current = current.held_for_second_step(hold.clone());
    }
    let ids = names.iter().map(|object| object.id).collect::<Vec<_>>();
    if orch_authz::nodes::holding(
        conn,
        &current,
        &ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await?
    .len()
        != ids.len()
    {
        return Ok(None);
    }
    Ok(Some(current))
}
