//! Private sender provenance for explicit notices, shared by HTTP and standalone delivery.
//! An opaque operation reference is not authority. Deferred source copies keep current original
//! credentials and native source checks. An ended move retains only its accepted personal receipt.

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
use orch_db::queries::notifications::operations::ending::{ChatMove, ThreadMove};
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
    /// The one common original raw HTTP identity could not be restored.
    #[error(transparent)]
    Raw(#[from] orch_authz::http_origin::Error),
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

struct CurrentSource {
    principal: Principal,
    raw: Option<orch_authz::http_origin::HttpOrigin>,
}
/// A delivery read is a bounded source context, never a reusable full mutation admission.
pub struct SourceRead {
    caller: Caller,
    until: Option<Time>,
}
impl SourceRead {
    /// Repeat the retained deadline at the final delivery clock cut.
    pub fn check(&self, clock: &dyn Clock) -> bool {
        self.caller.key_deadline_allows(clock) && self.until.is_none_or(|end| clock.time() < end)
    }
}
async fn current(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    subject: Subject,
    raw: Option<&orch_authz::http_origin::HttpOrigin>,
) -> Result<Option<CurrentSource>, Error> {
    if !transaction_guard::in_transaction(conn) {
        return Err(Error::Refused);
    }
    let copy = origin.operation() == "ChatDuplicate";
    let operation = Operation::of(origin.operation());
    if !copy && operation.is_none() {
        return Ok(None);
    }
    if copy && (!matches!(subject, Subject::Chat(_)) || raw.is_none()) {
        return Ok(None);
    }
    transaction_guard::lock_action(conn).await?;
    let Some(user) = transaction_guard::lock_native_actor(conn, origin.user()).await? else {
        return Ok(None);
    };
    let (caller, raw_until, raw_floor) = if let Some(raw) = raw {
        if raw.origin().operation() != origin.operation() || raw.origin().user() != origin.user() {
            return Ok(None);
        }
        let restored = match raw.current(conn, clock).await {
            Ok(value) => value,
            Err(
                orch_authz::http_origin::Error::Refused
                | orch_authz::http_origin::Error::InvalidOrigin,
            ) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let narrowed = raw.with_ceiling(restored.caller(), restored.until())?;
        (restored.caller().clone(), restored.until(), Some(narrowed))
    } else {
        let caller = match origin.restore_current(conn, clock).await {
            Ok(Some(caller)) => caller,
            Ok(None) | Err(orch_authz::Error::Stored(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        (caller, None, None)
    };
    if caller.user() != user.id
        || caller
            .grant()
            .is_some_and(|grant| grant.token_ids().next().is_none())
    {
        return Ok(None);
    }
    let plan = if copy {
        let Subject::Chat(id) = subject else {
            return Ok(None);
        };
        let row = chats::chat(conn, id).await?;
        if row.is_none_or(|row| row.kind != chats::KIND_GROUP) {
            return Ok(None);
        }
        let target = match key_targets::chat(conn, id).await {
            Ok(target) => target,
            Err(TargetError::Missing | TargetError::Authz(orch_authz::Error::NotFound(_))) => {
                return Ok(None);
            }
            Err(TargetError::Db(error)) => return Err(error.into()),
            Err(TargetError::Authz(error)) => return Err(error.into()),
        };
        orch_operation_scope::sources::bind_known(
            orch_api_contract::duplicates::ChatDuplicate::ROUTE,
            &orch_api_contract::duplicate_sources::CHAT,
            &[Facts::one(
                orch_api_contract::duplicate_sources::Source::Subject,
                orch_access_contract::key_scope::KeyTargetKind::Identity(
                    orch_access_contract::key_scope::KeyIdentityKind::Chat,
                ),
                target,
            )],
        )?
    } else {
        let operation = operation.ok_or(Error::Refused)?;
        let facts = match operation_facts(conn, operation, subject, true).await {
            Ok(facts) => facts,
            Err(TargetError::Missing | TargetError::Authz(orch_authz::Error::NotFound(_))) => {
                return Ok(None);
            }
            Err(TargetError::Db(error)) => return Err(error.into()),
            Err(TargetError::Authz(error)) => return Err(error.into()),
        };
        orch_operation_scope::sources::bind_required(
            operation.route(),
            operation.declaration(),
            &facts,
        )?
    };
    let caller = match if copy {
        plan.preparation_caller(&caller)
    } else {
        plan.admit(&caller, false)
    } {
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
    if let Some(hold) =
        orch_authz::http_origin::session_hold(conn, &user, raw.and_then(|raw| raw.session()))
            .await?
    {
        let facets = if copy {
            plan.known_required_facets()
        } else {
            plan.required_facets()?
        };
        let held = hold.orgs.iter().copied().collect::<BTreeSet<_>>();
        if facets.iter().any(|facet| {
            facet.target.node_ancestors.iter().any(|node| {
                node.kind == KeyTargetKind::Object(ObjectKind::Org)
                    && node.id.is_some_and(|id| held.contains(&id))
            })
        }) {
            return Ok(None);
        }
        principal = principal.held_for_second_step(hold);
    }
    if !reads(conn, &principal, subject).await?
        || !principal.caller().key_deadline_allows(clock)
        || raw_until.is_some_and(|end| clock.time() >= end)
    {
        return Ok(None);
    }
    let raw = raw_floor
        .map(|raw| raw.with_ceiling(principal.caller(), raw_until))
        .transpose()?;
    Ok(Some(CurrentSource { principal, raw }))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<orch_authz::http_origin::HttpOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accepted_raw: Option<orch_authz::http_origin::HttpOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_chat_move: Option<ChatMove>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_thread_move: Option<ThreadMove>,
}
impl Evidence {
    fn same_source(&self, other: &Self) -> bool {
        self.producer == other.producer
            && self.version == other.version
            && self.origin == other.origin
            && self.subject == other.subject
            && self.expected == other.expected
            && self.receivers == other.receivers
            && self.raw == other.raw
            && self.completed_chat_move == other.completed_chat_move
            && self.completed_thread_move == other.completed_thread_move
    }
    fn raw_floor(&self) -> Result<Option<orch_authz::http_origin::HttpOrigin>, Error> {
        match (&self.raw, &self.accepted_raw) {
            (None, None) => Ok(None),
            (Some(original), Some(accepted)) => Ok(Some(original.intersect(accepted)?)),
            _ => Err(Error::Refused),
        }
    }
}

/// A real accepted producer, kept private until all actual Inbox identities are known.
pub struct Capture {
    id: Id,
    origin: CapturedOperation,
    subject: Subject,
    expected: Option<Snapshot>,
    receivers: Vec<(Id, Id)>,
    raw: Option<orch_authz::http_origin::HttpOrigin>,
    completed_chat_move: Option<ChatMove>,
    completed_thread_move: Option<ThreadMove>,
    eligible: Vec<Id>,
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
        let principal = current(conn, clock, &origin, subject, None)
            .await?
            .ok_or(Error::Refused)?;
        Ok(Self {
            id,
            origin: CapturedOperation::capture(principal.principal.caller())
                .map_err(|_| Error::Refused)?,
            subject,
            expected: None,
            receivers: Vec::new(),
            raw: None,
            completed_chat_move: None,
            completed_thread_move: None,
            eligible: Vec::new(),
        })
    }

    /// Only the fully admitted native copy producer can attach a completed-copy source notice.
    pub async fn prepare_chat_copy(
        conn: &mut Conn,
        clock: &dyn Clock,
        accepted: &Caller,
        raw: orch_authz::http_origin::HttpOrigin,
        source: Id,
        created: Id,
        id: Id,
    ) -> Result<Self, Error> {
        if accepted.admitted_operation() != Some("ChatDuplicate")
            || raw.origin().operation() != "ChatDuplicate"
            || raw.origin().user() != accepted.user()
            || source == created
        {
            return Err(Error::Refused);
        }
        let new = chats::chat(conn, created).await?.ok_or(Error::Refused)?;
        if new.kind != chats::KIND_GROUP || new.created_by != Some(accepted.user()) {
            return Err(Error::Refused);
        }
        let origin = CapturedOperation::capture(accepted).map_err(|_| Error::Refused)?;
        let raw = raw.with_ceiling(accepted, raw.until())?;
        let current = current(conn, clock, &origin, Subject::Chat(source), Some(&raw))
            .await?
            .ok_or(Error::Refused)?;
        Ok(Self {
            id,
            origin,
            subject: Subject::Chat(source),
            expected: None,
            receivers: Vec::new(),
            raw: current.raw,
            completed_chat_move: None,
            completed_thread_move: None,
            eligible: Vec::new(),
        })
    }

    /// Capture the accepted native move before its old group ends. The receipt is
    /// delivered to those original members without reopening that spent source later.
    pub async fn prepare_chat_move(
        conn: &mut Conn,
        clock: &dyn Clock,
        accepted: &Caller,
        raw: orch_authz::http_origin::HttpOrigin,
        source: Id,
        receipt: &orch_api_contract::duplicates::DuplicateReceipt,
        id: Id,
    ) -> Result<Self, Error> {
        use orch_api_contract::duplicates::DuplicateKind;
        if !transaction_guard::in_transaction(conn)
            || accepted.admitted_operation() != Some("ChatMoveOut")
            || raw.origin().operation() != "ChatMoveOut"
            || raw.origin().user() != accepted.user()
            || receipt.kind != DuplicateKind::Chat
            || receipt.id == source
        {
            return Err(Error::Refused);
        }
        let raw = raw.with_ceiling(accepted, raw.until())?;
        let current = raw.current(conn, clock).await?;
        let caller = current
            .caller()
            .narrow_current_admission(accepted)
            .map_err(|_| Error::Refused)?;
        let origin = CapturedOperation::capture(&caller).map_err(|_| Error::Refused)?;
        if origin.operation() != "ChatMoveOut" {
            return Err(Error::Refused);
        }
        let old = chats::chat(conn, source).await?.ok_or(Error::Refused)?;
        let new = chats::chat(conn, receipt.id).await?.ok_or(Error::Refused)?;
        let members = chats::members(conn, source).await?;
        if old.kind != chats::KIND_GROUP
            || new.kind != chats::KIND_GROUP
            || old.org_id == new.org_id
            || new.created_by != Some(caller.user())
            || !members.iter().any(|member| {
                member.user_id == caller.user() && member.manager && member.active_at(clock.now())
            })
        {
            return Err(Error::Refused);
        }
        current.check(clock)?;
        Ok(Self {
            id,
            origin,
            subject: Subject::Chat(source),
            expected: None,
            receivers: Vec::new(),
            raw: Some(raw.with_ceiling(&caller, current.until())?),
            completed_chat_move: Some(ChatMove {
                source,
                created: receipt.id,
            }),
            completed_thread_move: None,
            eligible: members
                .into_iter()
                .filter(|member| member.active_at(clock.now()))
                .map(|member| member.user_id)
                .collect(),
        })
    }

    /// Capture an actual native thread-move source under its producer's complete current role proof.
    /// The engine validates the native job and supplies its original deadline at the Inbox writer.
    pub async fn prepare_thread_move(
        conn: &mut Conn,
        clock: &dyn Clock,
        caller: &Caller,
        receipt: &ThreadMove,
        people: &[Id],
    ) -> Result<Self, Error> {
        if !transaction_guard::in_transaction(conn)
            || caller.admitted_operation() != Some("ThreadMoveOut")
            || caller.admitted_required_roles().is_none()
            || !caller.key_deadline_allows(clock)
        {
            return Err(Error::Refused);
        }
        let old = orch_db::queries::threads::thread(conn, receipt.source)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(Error::Refused)?;
        let new = orch_db::queries::threads::thread(conn, receipt.created)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(Error::Refused)?;
        if old.org_id == new.org_id
            || new.owner_id != caller.user()
            || receipt.source == receipt.created
        {
            return Err(Error::Refused);
        }
        for user in people {
            let recipient = Principal::load(conn, Caller::session(*user), clock.time()).await?;
            if !reads(
                conn,
                &recipient,
                Subject::Object(ObjectRef {
                    kind: ObjectKind::Thread,
                    id: receipt.source,
                }),
            )
            .await?
            {
                return Err(Error::Refused);
            }
        }
        let origin = CapturedOperation::capture(caller).map_err(|_| Error::Refused)?;
        Ok(Self {
            id: receipt.job,
            origin,
            subject: Subject::Object(ObjectRef {
                kind: ObjectKind::Thread,
                id: receipt.source,
            }),
            expected: None,
            receivers: Vec::new(),
            raw: None,
            completed_chat_move: None,
            completed_thread_move: Some(*receipt),
            eligible: people.to_vec(),
        })
    }

    /// Recognize only the same already accepted factual native job receipt; never publish a new effect.
    pub async fn recorded_thread_move(
        conn: &mut Conn,
        receipt: &ThreadMove,
        by: Id,
    ) -> Result<bool, Error> {
        let Some(saved) = operations::get(conn, receipt.job).await? else {
            return Ok(false);
        };
        let saved: Evidence = serde_json::from_value(saved)?;
        if saved.completed_thread_move != Some(*receipt)
            || saved.origin.operation() != "ThreadMoveOut"
            || saved.origin.user() != by
        {
            return Err(Error::Refused);
        }
        Ok(true)
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
            || !self.matches_notice(notice)
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

    fn matches_notice(&self, notice: &NewNotification<'_>) -> bool {
        let ending = self
            .completed_chat_move
            .map(|value| {
                (
                    "completed_chat_move",
                    value.created,
                    "/chat/",
                    serde_json::json!(value),
                )
            })
            .or_else(|| {
                self.completed_thread_move.map(|value| {
                    (
                        "completed_thread_move",
                        value.created,
                        "/thread/",
                        serde_json::json!(value),
                    )
                })
            });
        if let Some((field, created, prefix, value)) = ending {
            return notice.node_id.is_none()
                && notice.thread_id.is_none()
                && notice.chat_id.is_none()
                && notice.approval_id.is_none()
                && notice.body.is_empty()
                && notice.data.as_object().is_some_and(|data| {
                    data.keys().all(|key| {
                        matches!(key.as_str(), "operation_id" | "by" | "subject" | "path")
                            || key == field
                    })
                })
                && self.eligible.contains(&notice.user_id)
                && notice.data.get(field) == Some(&value)
                && notice.data.get("path").and_then(Json::as_str)
                    == Some(format!("{prefix}{created}").as_str());
        }
        self.subject.matches_notice(notice)
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
                accepted_raw: self.raw.clone(),
                raw: self.raw,
                completed_chat_move: self.completed_chat_move,
                completed_thread_move: self.completed_thread_move,
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
pub struct Opened {
    id: Id,
    evidence: Arc<Evidence>,
    inbox: Id,
    receiver: Id,
    placement: Option<KeyResolvedTarget>,
    names: Vec<ObjectRef>,
    places: Vec<orch_db::queries::notifications::people::PlaceRow>,
    org_name: Option<String>,
}

/// The validated native source of one recipient's notification.
pub enum Loaded {
    /// An older notification uses its original native delivery rule.
    Legacy,
    /// The private source cannot disclose this notification.
    Closed,
    /// The same validated source is retained through external delivery.
    Source(Box<Opened>),
}

/// Validate the actual private producer, Inbox association and current disclosure source.
pub async fn load(
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
    if evidence.completed_chat_move.is_some() || evidence.completed_thread_move.is_some() {
        let opened = Opened {
            id,
            evidence: Arc::new(evidence),
            inbox: snapshot.id.ok_or(Error::Refused)?,
            receiver,
            placement: None,
            names: Vec::new(),
            places: Vec::new(),
            org_name: None,
        };
        return Ok(if opened.check(conn, clock).await?.is_some() {
            Loaded::Source(Box::new(opened))
        } else {
            Loaded::Closed
        });
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
        placement: Some(placement),
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
    async fn current_source(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<CurrentSource>, Error> {
        let Some(before) = operations::lock(conn, self.id).await? else {
            return Ok(None);
        };
        let Ok(evidence) = serde_json::from_value::<Evidence>(before.clone()) else {
            return Ok(None);
        };
        if !evidence.same_source(&self.evidence) {
            return Ok(None);
        }
        let mut raw = match evidence.raw_floor() {
            Ok(raw) => raw,
            Err(Error::Refused | Error::Raw(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        if let (Some(current), Some(old)) = (&raw, self.evidence.raw_floor()?) {
            raw = Some(current.intersect(&old)?);
        }
        let Some(current) = current(
            conn,
            clock,
            &evidence.origin,
            evidence.subject,
            raw.as_ref(),
        )
        .await?
        else {
            return Ok(None);
        };
        if let Some(accepted) = &current.raw
            && evidence.accepted_raw.as_ref() != Some(accepted)
            && !operations::retain_raw(
                conn,
                self.id,
                &before,
                &serde_json::to_value(accepted)?,
                clock.now(),
            )
            .await?
        {
            return Ok(None);
        }
        if !current.principal.caller().key_deadline_allows(clock)
            || current
                .raw
                .as_ref()
                .and_then(|raw| raw.until())
                .is_some_and(|end| clock.time() >= end)
        {
            return Ok(None);
        }
        Ok(Some(current))
    }

    /// The actual recipient Inbox identity.
    pub fn inbox(&self) -> Id {
        self.inbox
    }
    /// Only the source names this projection admitted.
    pub fn places(&self) -> &[orch_db::queries::notifications::people::PlaceRow] {
        &self.places
    }
    /// The admitted original organization caption, when present.
    pub fn org_name(&self) -> Option<&str> {
        self.org_name.as_deref()
    }

    async fn project_names(&mut self, conn: &mut Conn, clock: &dyn Clock) -> Result<bool, Error> {
        let Some(current) = self.current_source(conn, clock).await? else {
            return Ok(false);
        };
        let principal = current.principal;
        let placement = self.placement.as_ref().ok_or(Error::Refused)?;
        let ids: Vec<_> = placement
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
    pub async fn check(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<SourceRead>, Error> {
        if self.evidence.completed_chat_move.is_some()
            || self.evidence.completed_thread_move.is_some()
        {
            return self.check_completed_move(conn, clock).await;
        }
        let Some(current) = self.current_source(conn, clock).await? else {
            return Ok(None);
        };
        let until = current.raw.as_ref().and_then(|raw| raw.until());
        let principal = current.principal;
        let facts = match facts(conn, self.evidence.subject).await {
            Ok(facts) => facts,
            Err(TargetError::Missing | TargetError::Authz(orch_authz::Error::NotFound(_))) => {
                return Ok(None);
            }
            Err(TargetError::Db(error)) => return Err(error.into()),
            Err(TargetError::Authz(error)) => return Err(error.into()),
        };
        if Some(&facts[0].groups[0].1[0]) != self.placement.as_ref() {
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
        Ok((principal.caller().key_deadline_allows(clock)
            && until.is_none_or(|end| clock.time() < end))
        .then(|| SourceRead {
            caller: principal.caller().clone(),
            until,
        }))
    }

    async fn check_completed_move(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<SourceRead>, Error> {
        let Some(saved) = operations::lock(conn, self.id).await? else {
            return Ok(None);
        };
        let Ok(saved) = serde_json::from_value::<Evidence>(saved) else {
            return Ok(None);
        };
        if !saved.same_source(&self.evidence)
            || !operations::ending::move_readable(
                conn,
                self.receiver,
                &saved.expected.kind,
                &saved.expected.data,
            )
            .await?
        {
            return Ok(None);
        }
        // This is the existing observational recipient door, never a reconstructed sender.
        let recipient =
            match Principal::load(conn, Caller::session(self.receiver), clock.time()).await {
                Ok(recipient) => recipient,
                Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
        Ok(Some(SourceRead {
            caller: recipient.caller().clone(),
            until: None,
        }))
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

#[cfg(test)]
mod thread_move_tests;
