//! Exact original-operation sources at the native adoption preparation boundaries.

use super::{AdoptError, Adoption, RetainedHistory};
use crate::commands::{CommandAuthority, Outgoing};
use crate::operation_scope::{
    Prepared,
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use crate::{EngineError, access::resource_policies, binding::NewThread, harnesses::HarnessKey};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::{Decision, ObjectRef},
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    adoption_sources::{self as declarations, Source as Role},
    nodes::{ThreadAdopt, ThreadAdoptPreview},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    threads::ThreadAdoptRetryHistory,
};
use orch_authz::{Act, Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        hierarchy,
        link::{CommandRow, NewCommand},
        resources,
        resources::thread_engine as rows,
    },
};
use orch_link_contract::commands::{
    AgentDiscoverSessions, AgentReadSession, AgentStart, CommandKind, SerialKey, UnitAfterUp,
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod record;
pub(super) use record::RecordObservation;

const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);
const AGENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Agent);
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);
const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

/// A known original preparation source closed; absence never becomes ordinary work.
pub use crate::harnesses::preparation::Ended as PreparationEnded;
fn engine(error: Error) -> EngineError {
    match error {
        Error::Refused | Error::Binding(crate::operation_scope::BindingError::Refused) => {
            EngineError::BeforeEffect(Box::new(PreparationEnded))
        }
        Error::Native(error) => error,
        Error::Binding(error) => EngineError::unreadable(error),
    }
}
fn ordinary(error: AdoptError) -> Error {
    match error {
        AdoptError::Internal(error) => Error::Native(error),
        _ => Error::Refused,
    }
}

/// Only the actual native entrypoint selects the operation, never a credential label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Purpose {
    Adopt,
    Preview,
    RetryHistory,
}
impl Purpose {
    fn of(caller: &Caller) -> Result<Self, EngineError> {
        match caller.admitted_operation() {
            Some("ThreadAdopt") => Ok(Self::Adopt),
            Some("ThreadAdoptPreview") => Ok(Self::Preview),
            Some("ThreadAdoptRetryHistory") => Ok(Self::RetryHistory),
            _ => Err(EngineError::unreadable(
                "native adoption preparation has no exact admitted entrypoint",
            )),
        }
    }
}

/// The shared actual raw request credential; the private stored representation is unchanged.
pub use crate::operation_scope::request_origin::Credential as HttpCredential;

/// Original credential and factual request, without a fabricated browser-session lease.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preparation {
    origin: CapturedOperation,
    credential: HttpCredential,
    purpose: Purpose,
    adoption: Adoption,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_retained"
    )]
    retained: Option<RetainedHistory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    http_until: Option<Time>,
    #[serde(skip)]
    transport: Option<Arc<dyn Transport>>,
}
impl PartialEq for Preparation {
    fn eq(&self, other: &Self) -> bool {
        self.origin == other.origin
            && self.credential == other.credential
            && self.purpose == other.purpose
            && self.adoption == other.adoption
            && self.retained == other.retained
            && self.http_until == other.http_until
    }
}
fn present_retained<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RetainedHistory>, D::Error> {
    RetainedHistory::deserialize(deserializer).map(Some)
}

impl std::fmt::Debug for Preparation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdoptionPreparation([private])")
    }
}

impl Preparation {
    /// The Registry's initial provider supplies its actual known-source caller and exact raw credential association.
    pub(crate) fn capture(
        caller: &Caller,
        credential: HttpCredential,
        adoption: Adoption,
    ) -> Result<Self, EngineError> {
        if !credential.matches(caller) {
            return Err(EngineError::unreadable(
                "native adoption has another actual HTTP credential",
            ));
        }
        let purpose = Purpose::of(caller)?;
        if purpose == Purpose::RetryHistory {
            return Err(EngineError::unreadable(
                "retained history retry needs its original marker source",
            ));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            credential,
            purpose,
            adoption,
            retained: None,
            http_until: None,
            transport: None,
        })
    }
    pub(crate) fn capture_retry(
        caller: &Caller,
        credential: HttpCredential,
        retained: RetainedHistory,
    ) -> Result<Self, EngineError> {
        if Purpose::of(caller)? != Purpose::RetryHistory || !credential.matches(caller) {
            return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            credential,
            purpose: Purpose::RetryHistory,
            adoption: retained.adoption(),
            retained: Some(retained),
            http_until: None,
            transport: None,
        })
    }
    /// The actual HTTP producer installs its exact bridge before any native setup.
    pub(crate) fn with_transport(
        mut self,
        transport: Option<Arc<dyn Transport>>,
        until: Option<Time>,
    ) -> Self {
        self.http_until = minimum(self.http_until, until);
        self.transport = transport;
        self
    }
    /// Freeze the actual original/current minimum at this native phase's first accepted effect.
    pub(crate) fn accepted_at(&self, check: &Checkpoint) -> Result<Self, EngineError> {
        let mut accepted = self.clone();
        accepted.origin =
            CapturedOperation::capture(&check.caller).map_err(EngineError::unreadable)?;
        accepted.http_until = minimum(self.http_until, check.until);
        Ok(accepted)
    }
    /// Keep transport contexts out of retained results and private command envelopes.
    pub(crate) fn without_transport(mut self) -> Self {
        self.transport = None;
        self
    }
    /// Only a genuine recorded browser Session supplies this policy input.
    pub(crate) fn browser_session(&self) -> Option<Id> {
        self.credential.browser_session()
    }
    /// The immutable native request, never restored permission.
    pub fn adoption(&self) -> &Adoption {
        &self.adoption
    }
    /// The actual original operation for the shared HTTP/current-source bridge.
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }

    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<(Caller, Option<Time>), Error> {
        crate::operation_scope::request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.http_until,
            match self.purpose {
                Purpose::Adopt => "ThreadAdopt",
                Purpose::Preview => "ThreadAdoptPreview",
                Purpose::RetryHistory => "ThreadAdoptRetryHistory",
            },
        )
        .await
    }

    /// Check the complete actual setup footprint on the writer's own open transaction.
    /// This authorizes preparation only; future Environment and Thread sources stay unknown.
    pub(crate) async fn checkpoint(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        key: HarnessKey,
    ) -> Result<Checkpoint, EngineError> {
        self.checkpoint_for(conn, clock, key, None)
            .await
            .map_err(engine)
    }
    async fn checkpoint_for(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        key: HarnessKey,
        command: Option<NativeCommand>,
    ) -> Result<Checkpoint, Error> {
        let input = Input {
            request: self.clone(),
            key,
            command,
        };
        if let Some(transport) = &self.transport {
            return transport
                .checkpoint(conn, input)
                .await
                .map_err(Error::Native);
        }
        let (caller, until) = self.current(conn, clock).await?;
        let (caller, first) = match self.purpose {
            Purpose::Adopt => {
                let frame = continuing::preparation::prepare_with_caller::<Phase<Adopting>>(
                    conn,
                    clock,
                    &self.origin,
                    &caller,
                    input,
                )
                .await?;
                let parts = frame.into_parts();
                (parts.principal.caller().clone(), parts.first_effect)
            }
            Purpose::Preview => {
                let frame = continuing::preparation::prepare_with_caller::<Phase<Previewing>>(
                    conn,
                    clock,
                    &self.origin,
                    &caller,
                    input,
                )
                .await?;
                let parts = frame.into_parts();
                (parts.principal.caller().clone(), parts.first_effect)
            }
            Purpose::RetryHistory => {
                let frame = continuing::preparation::prepare_with_caller::<Phase<Retrying>>(
                    conn,
                    clock,
                    &self.origin,
                    &caller,
                    input,
                )
                .await?;
                let parts = frame.into_parts();
                (parts.principal.caller().clone(), parts.first_effect)
            }
        };
        Ok(Checkpoint {
            caller,
            until,
            first,
            transport: None,
        })
    }

    /// Attach exact private evidence to this server-built command, without touching its wire bytes.
    pub(crate) async fn command(
        &self,
        engine: &crate::Engine,
        key: HarnessKey,
        mut command: Outgoing,
    ) -> Result<Outgoing, EngineError> {
        if command.authority.is_some() {
            return Err(EngineError::unreadable(
                "adoption preparation cannot replace another native command origin",
            ));
        }
        let native = NativeCommand::from_outgoing(&command);
        let mut tx = engine.inner.db.begin().await?;
        lock_retained_source(&mut tx, self).await?;
        let _ = sqlx::query("SELECT id FROM commands WHERE id=$1 FOR SHARE")
            .bind(command.id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?;
        // Native shared setup can resume an already queued command. Its exact canonical
        // envelope and private origin stay authoritative, including genuine old absence.
        // The dispatcher rechecks unaccepted work; an accepted receipt is reconciliation.
        if let Some(row) = orch_db::queries::link::command(&mut tx, command.id).await? {
            if !native.matches(
                row.id,
                row.machine_id,
                &row.kind,
                row.serial_key(),
                &row.body,
            ) {
                return Err(crate::commands::CommandsError::AuthorityMismatch.into());
            }
            command.authority = orch_db::queries::link::command_authority(&mut tx, row.id)
                .await?
                .map(serde_json::from_value)
                .transpose()?;
            tx.commit().await.map_err(DbError::from)?;
            return Ok(command);
        }
        let authority = self
            .command_on(&mut tx, engine.inner.clock.as_ref(), key, &command)
            .await?;
        tx.commit().await.map_err(DbError::from)?;
        command.authority = Some(authority);

        Ok(command)
    }

    async fn command_on(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        key: HarnessKey,
        command: &Outgoing,
    ) -> Result<CommandAuthority, EngineError> {
        let native = NativeCommand::from_outgoing(command);
        let check = self
            .checkpoint_for(conn, clock, key, Some(native.clone()))
            .await
            .map_err(engine_error)?;
        let request = self.accepted_at(&check)?.without_transport();
        check.check(clock)?;
        Ok(CommandAuthority::adoption_preparation(CommandEvidence {
            request,
            key,
            command: native,
        }))
    }
}
impl crate::harnesses::preparation::Source for Preparation {
    fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    fn until(&self) -> Option<Time> {
        self.http_until
    }
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: crate::harnesses::preparation::Target,
    ) -> BoxFuture<'a, Result<crate::harnesses::preparation::Checkpoint, EngineError>> {
        Box::pin(async move {
            let crate::harnesses::preparation::Target::Harness(key) = target else {
                return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
            };
            let check = Preparation::checkpoint(self, conn, clock, key).await?;
            Ok(crate::harnesses::preparation::Checkpoint::from_check(
                check.caller.clone(),
                check.until,
                Box::new(move |clock| check.check(clock)),
            ))
        })
    }
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Arc<dyn crate::harnesses::preparation::Source>, EngineError> {
        if origin.user() != self.origin.user() || origin.operation() != self.origin.operation() {
            return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
        }
        let mut request = self.clone();
        request.origin = origin;
        request.http_until = minimum(request.http_until, until);
        Ok(Arc::new(request))
    }
    fn command<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: crate::harnesses::preparation::Target,
        command: &'a Outgoing,
    ) -> BoxFuture<'a, Result<CommandAuthority, EngineError>> {
        Box::pin(async move {
            let crate::harnesses::preparation::Target::Harness(key) = target else {
                return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
            };
            self.command_on(conn, clock, key, command).await
        })
    }
}

fn engine_error(error: Error) -> EngineError {
    engine(error)
}

/// The native preparation first-effect seam, carrying a real shared phase proof.
pub struct Checkpoint {
    caller: Caller,
    until: Option<Time>,
    first: continuing::preparation::FirstEffect,
    transport: Option<BeforeTransport>,
}
/// The exact raw HTTP deadline check, supplied by that request's shared bridge.
pub use crate::harnesses::preparation::BeforeEffect as BeforeTransport;
impl Checkpoint {
    /// Accept only an actual preparation Frame of this closed native source family.
    pub fn from_frame<E: Entry>(
        frame: continuing::preparation::Frame<Phase<E>>,
        until: Option<Time>,
        before: BeforeTransport,
    ) -> Self {
        let parts = frame.into_parts();
        Self {
            caller: parts.principal.caller().clone(),
            until: minimum(parts.input.request.http_until, until),
            first: parts.first_effect,
            transport: Some(before),
        }
    }
    /// Actual current native actor, still restricted by every retained credential ceiling.
    pub fn caller(&self) -> &Caller {
        &self.caller
    }
    /// Actual original/current raw credential minimum for a later rollbackable tail check.
    pub fn until(&self) -> Option<Time> {
        self.until
    }
    /// Consume the real source cut at the native writer's first effect on this same transaction.
    pub fn check(self, clock: &dyn Clock) -> Result<(), EngineError> {
        if !self.caller.key_deadline_allows(clock)
            || self
                .until
                .is_some_and(|end| clock.time().get() >= end.get())
        {
            return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
        }
        if let Some(before) = self.transport {
            before()?;
        }
        self.first.check(clock).map_err(engine)
    }
}
fn minimum(a: Option<Time>, b: Option<Time>) -> Option<Time> {
    match (a, b) {
        (Some(a), Some(b)) => Some(std::cmp::min(a, b)),
        (a, b) => a.or(b),
    }
}

/// A domain native writer asks its actual transport to compose the shared HTTP/native frame.
/// No context is serialized: queued commands keep only captured origin, native facts and the ceiling.
pub trait Transport: Send + Sync {
    /// Runs on the actual setup/queue transaction, after its native waits and before its first effect.
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        input: Input,
    ) -> BoxFuture<'a, Result<Checkpoint, EngineError>>;
}

fn factual_target(error: targets::TargetError, id: Id) -> EngineError {
    match error {
        targets::TargetError::Missing => EngineError::Authz(orch_authz::Error::NotFound(id)),
        targets::TargetError::Db(error) => error.into(),
        targets::TargetError::Authz(error) => error.into(),
    }
}

/// Actual Topic/Provider/Agent sources used by the preparation, before discovering an Environment.
pub async fn initial_facts(
    conn: &mut Conn,
    adoption: &Adoption,
) -> Result<Vec<Facts<Role>>, EngineError> {
    let topic = targets::node(conn, adoption.topic)
        .await
        .map_err(|error| factual_target(error, adoption.topic))?;
    if topic.object.kind != KeyTargetKind::Object(ObjectKind::Topic) {
        return Err(EngineError::unreadable(
            "native adoption needs a real Topic",
        ));
    }
    let provider = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: adoption.provider,
        },
    )
    .await
    .map_err(|error| factual_target(error, adoption.provider))?;
    let agent = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Agent,
            id: adoption.agent,
        },
    )
    .await
    .map_err(|error| factual_target(error, adoption.agent))?;
    Ok(vec![
        Facts::one(Role::Destination, THREAD, topic),
        Facts::one(Role::Session, PROVIDER, provider.clone()),
        Facts::collection(
            Role::Binding,
            vec![(PROVIDER, vec![provider]), (AGENT, vec![agent])],
        ),
    ])
}

/// Actual retained Thread and Provider facts, resolved from the original adopted marker.
pub async fn retry_history_facts(
    conn: &mut Conn,
    retained: &RetainedHistory,
) -> Result<Vec<Facts<Role>>, EngineError> {
    if !retained
        .current(conn)
        .await
        .map_err(EngineError::unreadable)?
    {
        return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
    }
    let thread = targets::node(conn, retained.thread())
        .await
        .map_err(EngineError::unreadable)?;
    if thread.object.kind != THREAD {
        return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
    }
    let provider = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: retained.provider(),
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    Ok(vec![
        Facts::one(Role::RetainedThread, THREAD, thread),
        Facts::one(Role::Session, PROVIDER, provider),
    ])
}

/// The existing ordinary prerequisites with the supplied current native actor, not a user-id Session.
pub async fn require_initial(
    conn: &mut Conn,
    principal: &Principal,
    adoption: &Adoption,
) -> Result<(), AdoptError> {
    let topic = orch_authz::nodes::check(
        conn,
        principal,
        adoption.topic,
        Permission::TopicStartThreads,
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(EngineError::from)?;
    if let Decision::Refused(refusal) = topic {
        return Err(super::CreateThreadError::Denied(Box::new(refusal)).into());
    }
    for (kind, id, permission) in [
        (ObjectKind::Agent, adoption.agent, Permission::AgentUse),
        (
            ObjectKind::Provider,
            adoption.provider,
            Permission::ProviderUse,
        ),
    ] {
        if !principal.caller().permits(permission) {
            return Err(super::denied(permission, kind, id, Vec::new()));
        }
        let policy = resource_policies(conn, id).await?;
        match orch_authz::usable::usable(conn, principal, id, Some(adoption.topic), &policy)
            .await
            .map_err(EngineError::from)?
        {
            orch_authz::usable::Usable::Yes(_) | orch_authz::usable::Usable::Unavailable(_) => {}
            _ => {
                let decision = orch_authz::resources::check(
                    conn,
                    principal,
                    ObjectRef { kind, id },
                    permission,
                    policy.sharing_at(None),
                )
                .await
                .map_err(EngineError::from)?;
                let missing = match decision {
                    Decision::Refused(refusal) => refusal.missing,
                    _ => Vec::new(),
                };
                return Err(super::denied(permission, kind, id, missing));
            }
        }
    }
    Ok(())
}

/// The native place selector's current actor and fixed source row, loaded once per selection.
/// Its partial known-source caller is private and can only answer ordinary candidate queries.
pub(super) struct PlaceFilter {
    principal: Principal,
    plan: Prepared,
    slot: usize,
}
impl PlaceFilter {
    pub(super) async fn prepare(
        conn: &mut Conn,
        original: &Principal,
        adoption: &Adoption,
        selection: &str,
        now: Time,
    ) -> Result<Self, AdoptError> {
        let (route, declaration) = match selection {
            "ThreadAdopt" => (ThreadAdopt::ROUTE, &declarations::ADOPT_PREPARATION),
            "ThreadAdoptPreview" => (
                ThreadAdoptPreview::ROUTE,
                &declarations::PREVIEW_PREPARATION,
            ),
            _ => {
                return Err(EngineError::unreadable(
                    "native adoption has another place-selection purpose",
                )
                .into());
            }
        };
        let facts = initial_facts(conn, adoption).await?;
        let plan = crate::operation_scope::sources::bind_known(route, declaration, &facts)
            .map_err(EngineError::unreadable)?;
        let binding = declaration
            .bindings
            .iter()
            .find(|binding| binding.source == Role::Binding)
            .ok_or_else(|| EngineError::unreadable("native adoption has no binding role"))?;
        let indices = crate::operation_scope::sources::indices(&plan, binding)
            .map_err(EngineError::unreadable)?;
        let [slot] = indices.as_slice() else {
            return Err(
                EngineError::unreadable("native adoption has another binding cardinality").into(),
            );
        };
        let caller = plan
            .preparation_caller(original.caller())
            .map_err(scope_for_place)?;
        let principal = Principal::load(conn, caller, now)
            .await
            .map_err(EngineError::from)?;
        let principal = match original.second_step_hold() {
            Some(hold) => principal.held_for_second_step(hold.clone()),
            None => principal,
        };
        Ok(Self {
            principal,
            plan,
            slot: *slot,
        })
    }
    pub(super) async fn allows(&self, conn: &mut Conn, environment: Id) -> Result<(), AdoptError> {
        let target = targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
        )
        .await
        .map_err(EngineError::unreadable)?;
        if !self
            .plan
            .permits_binding(self.principal.caller(), self.slot, ENVIRONMENT, &[target])
            .map_err(scope_for_place)?
        {
            return Err(crate::binding::CreateThreadError::OperationScope.into());
        }
        let policy = resource_policies(conn, environment).await?;
        let decision = orch_authz::resources::check(
            conn,
            &self.principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
            Permission::EnvironmentUse,
            policy.sharing_at(None),
        )
        .await
        .map_err(EngineError::from)?;
        if let Decision::Refused(refusal) = decision {
            return Err(crate::binding::CreateThreadError::Denied(Box::new(refusal)).into());
        }
        Ok(())
    }
}
pub(super) fn scope_for_place(
    error: crate::operation_scope::BindingError,
) -> crate::binding::CreateThreadError {
    match error {
        crate::operation_scope::BindingError::Refused => {
            crate::binding::CreateThreadError::OperationScope
        }
        error => crate::binding::CreateThreadError::Internal(EngineError::unreadable(error)),
    }
}

/// Factual actual binding for the one common creator completion; it supplies no admission.
pub async fn bind_creation(
    conn: &mut Conn,
    plan: &mut Prepared,
    new: &NewThread,
) -> Result<(), crate::binding::CreateThreadError> {
    use orch_api_contract::operations::TargetResolver;
    let slot = |plan: &Prepared, role: &str| {
        plan.variant().facets.iter().position(|facet|matches!(facet.target,TargetResolver::Resolved(name)|TargetResolver::ResolvedCollection(name) if name == role)).ok_or_else(||crate::binding::CreateThreadError::Internal(EngineError::unreadable("the native adoption role is not declared")))
    };
    let provider = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: new.provider,
        },
    )
    .await
    .map_err(|error| crate::binding::CreateThreadError::Internal(EngineError::unreadable(error)))?;
    let agent = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Agent,
            id: new.agent,
        },
    )
    .await
    .map_err(|error| crate::binding::CreateThreadError::Internal(EngineError::unreadable(error)))?;
    let environment = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: new.environment,
        },
    )
    .await
    .map_err(|error| crate::binding::CreateThreadError::Internal(EngineError::unreadable(error)))?;
    plan.bind(
        slot(plan, "native_session")?,
        PROVIDER,
        std::slice::from_ref(&provider),
    )
    .map_err(|error| crate::binding::CreateThreadError::Internal(EngineError::unreadable(error)))?;
    plan.bind_collection(
        slot(plan, "chosen_binding")?,
        &[
            (PROVIDER, vec![provider]),
            (AGENT, vec![agent]),
            (ENVIRONMENT, vec![environment]),
        ],
    )
    .map_err(|error| crate::binding::CreateThreadError::Internal(EngineError::unreadable(error)))?;
    Ok(())
}

pub(super) async fn lock_sources(
    conn: &mut Conn,
    topic: Id,
    key: HarnessKey,
) -> Result<bool, EngineError> {
    if !hierarchy::lock(conn, topic).await? {
        return Ok(false);
    }
    let mut ids = [(key.provider, "provider"), (key.agent, "agent")];
    ids.sort_unstable();
    for (id, kind) in ids {
        if !resources::lock_resource(conn, kind, id).await? {
            return Ok(false);
        }
    }
    crate::harnesses::claim_lock(conn, key).await?;
    Ok(true)
}

async fn lock_retained_source(conn: &mut Conn, request: &Preparation) -> Result<(), EngineError> {
    let Some(retained) = request.retained.as_ref() else {
        return Ok(());
    };
    if request.purpose != Purpose::RetryHistory
        || orch_db::queries::transaction_guard::lock_native_actor(conn, request.origin.user())
            .await?
            .is_none()
        || orch_db::queries::threads::lock(conn, retained.thread())
            .await?
            .is_none()
    {
        return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
    }
    let marker: Option<Id> = sqlx::query_scalar(
        "SELECT id FROM items WHERE id=$1 AND thread_id=$2 AND segment_id=$3 FOR SHARE",
    )
    .bind(retained.marker())
    .bind(retained.thread())
    .bind(retained.segment())
    .fetch_optional(conn)
    .await
    .map_err(DbError::from)?;
    if marker.is_none() {
        return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
    }
    Ok(())
}

/// Immutable native operands passed to the exact HTTP preparation bridge.
#[derive(Clone)]
pub struct Input {
    request: Preparation,
    key: HarnessKey,
    command: Option<NativeCommand>,
}
impl Input {
    /// The actual original operation, never a new caller constructed from a user id.
    pub fn origin(&self) -> &CapturedOperation {
        &self.request.origin
    }
}
#[derive(PartialEq)]
/// Current native association; no fields are transport authority.
pub struct Operands {
    profile: Json,
    configuration: Json,
    machine: Id,
    orgs: std::collections::BTreeSet<Id>,
}
/// The closed native preparation entrypoints.
pub trait Entry: sealed::Entry + Send + Sync + 'static {
    const ROUTE: Route;
    const DECLARATION: &'static Declaration<Role>;
}
/// Known-source preparation of the actual adopting creator.
pub struct Adopting;
/// Known-source preparation of its independent preview.
pub struct Previewing;
/// Known retained Thread, Provider and original marker source of missing-history retry.
pub struct Retrying;
mod sealed {
    pub trait Entry {}
    impl Entry for super::Adopting {}
    impl Entry for super::Previewing {}
    impl Entry for super::Retrying {}
}
impl Entry for Adopting {
    const ROUTE: Route = ThreadAdopt::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::ADOPT_PREPARATION;
}
impl Entry for Previewing {
    const ROUTE: Route = ThreadAdoptPreview::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::PREVIEW_PREPARATION;
}
impl Entry for Retrying {
    const ROUTE: Route = ThreadAdoptRetryHistory::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::RETRY_PREPARATION;
}
/// Native preparation purpose, disjoint from the complete operation frame.
pub struct Phase<E>(std::marker::PhantomData<E>);
impl<E: Entry> Source for Phase<E> {
    type Role = Role;
    type Input = Input;
    type Value = Operands;
    const ROUTE: Route = E::ROUTE;
    const DECLARATION: &'static Declaration<Role> = E::DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        _: Time,
    ) -> BoxFuture<'a, Result<(Operands, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let adoption = &input.request.adoption;
            if input.request.origin.operation() != E::DECLARATION.operation
                || input.key.provider != adoption.provider
                || input.key.agent != adoption.agent
            {
                return Err(Error::Refused);
            }
            let provider = rows::provider(conn, adoption.provider)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(Error::Refused)?;
            let agent = rows::agent(conn, adoption.agent)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(Error::Refused)?;
            if !crate::harnesses::places_on(conn, &provider, adoption.agent)
                .await?
                .contains(&input.key.machine)
                || !rows::machine_place(conn, input.key.machine)
                    .await?
                    .is_some_and(|row| row.deleted_at.is_none() && row.status == "active")
            {
                return Err(Error::Refused);
            }
            if let Some(command) = &input.command {
                command
                    .require_native(conn, input.key, adoption, input.request.purpose)
                    .await?;
            }
            let facts = match (input.request.purpose, input.request.retained.as_ref()) {
                (Purpose::RetryHistory, Some(retained)) if retained.adoption() == *adoption => {
                    retry_history_facts(conn, retained).await?
                }
                (Purpose::Adopt | Purpose::Preview, None) => initial_facts(conn, adoption).await?,
                _ => return Err(Error::Refused),
            };
            let orgs = facts
                .iter()
                .flat_map(|fact| &fact.groups)
                .flat_map(|(_, targets)| targets)
                .flat_map(|target| &target.node_ancestors)
                .filter(|node| node.kind == KeyTargetKind::Object(ObjectKind::Org))
                .filter_map(|node| node.id)
                .collect();
            let value = Operands {
                profile: serde_json::json!({"kind":agent.harness_kind,"profile":agent.profile}),
                configuration: crate::harnesses::provider_config_of(conn, adoption.provider)
                    .await?,
                machine: input.key.machine,
                orgs,
            };
            Ok((value, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        value: &'a Operands,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let session = match &input.request.credential {
                HttpCredential::Session { id, .. } => Some(*id),
                HttpCredential::Token { .. } | HttpCredential::Native => None,
            };
            if let Some(session) = session {
                let user = orch_db::queries::identity::user_by_id(conn, principal.user())
                    .await?
                    .ok_or(Error::Refused)?;
                if let Some(hold) =
                    crate::operation_scope::account_policy::session_hold(conn, &user, Some(session))
                        .await?
                    && hold.orgs.iter().any(|org| value.orgs.contains(org))
                {
                    return Err(Error::Refused);
                }
            }
            match (input.request.purpose, input.request.retained.as_ref()) {
                (Purpose::RetryHistory, Some(retained)) => {
                    super::require_retry_history(conn, principal, retained.thread())
                        .await
                        .map_err(ordinary)
                }
                (Purpose::Adopt | Purpose::Preview, None) => {
                    require_initial(conn, principal, &input.request.adoption)
                        .await
                        .map_err(ordinary)
                }
                _ => Err(Error::Refused),
            }
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        _: &'a Operands,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if input.request.retained.is_some() {
                lock_retained_source(conn, &input.request).await?;
                let mut ids = [(input.key.provider, "provider"), (input.key.agent, "agent")];
                ids.sort_unstable();
                for (id, kind) in ids {
                    if !resources::lock_resource(conn, kind, id).await? {
                        return Err(Error::Refused);
                    }
                }
                crate::harnesses::claim_lock(conn, input.key).await?;
                return Ok(());
            }
            if !lock_sources(conn, input.request.adoption.topic, input.key).await? {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a Operands,
        fresh: &'a Operands,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeCommand {
    id: Id,
    machine: Id,
    kind: CommandKind,
    serial: Option<String>,
    body: Json,
}
impl NativeCommand {
    fn from_outgoing(command: &Outgoing) -> Self {
        Self {
            id: command.id,
            machine: command.machine,
            kind: command.kind,
            serial: command.key.wire(),
            body: command.body.clone(),
        }
    }
    async fn require_native(
        &self,
        conn: &mut Conn,
        key: HarnessKey,
        adoption: &Adoption,
        purpose: Purpose,
    ) -> Result<(), Error> {
        if self.id.is_nil() || self.machine != key.machine {
            return Err(Error::Refused);
        }
        let harness = SerialKey::Harness {
            provider_id: key.provider,
            agent_id: key.agent,
        }
        .wire();
        let expected = match self.kind {
            CommandKind::AgentStart => {
                let frozen: Option<Json> = sqlx::query_scalar("SELECT after_up_plan->'request' FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3 AND start_command_id=$4").bind(key.provider).bind(key.agent).bind(key.machine).bind(self.id).fetch_optional(&mut *conn).await.map_err(DbError::from)?.flatten();
                if self.serial != harness {
                    return Err(Error::Refused);
                }
                let frozen = frozen
                    .filter(|value| !value.is_null())
                    .ok_or(Error::Refused)?;
                let body: AgentStart =
                    serde_json::from_value(frozen.clone()).map_err(EngineError::from)?;
                let profile = rows::agent(conn, key.agent).await?.ok_or(Error::Refused)?;
                if body.provider_id != key.provider
                    || body.agent_id != key.agent
                    || body.harness_kind != profile.harness_kind
                {
                    return Err(Error::Refused);
                }
                frozen
            }
            CommandKind::AgentDiscoverSessions => {
                if self.serial != harness {
                    return Err(Error::Refused);
                }
                serde_json::to_value(AgentDiscoverSessions {
                    provider_id: key.provider,
                    agent_id: key.agent,
                })
                .map_err(EngineError::from)?
            }
            CommandKind::AgentReadSession => {
                if self.serial != harness {
                    return Err(Error::Refused);
                }
                serde_json::to_value(AgentReadSession {
                    provider_id: key.provider,
                    agent_id: key.agent,
                    native_session_ref: adoption.native_session_ref.clone(),
                    write: purpose != Purpose::Preview,
                })
                .map_err(EngineError::from)?
            }
            CommandKind::UnitAfterUp => {
                let body: UnitAfterUp =
                    serde_json::from_value(self.body.clone()).map_err(EngineError::from)?;
                let plan = crate::harnesses::retained_attach_command(conn, key, &body)
                    .await?
                    .ok_or(Error::Refused)?;
                if self.serial != SerialKey::Runtime(key.provider).wire()
                    || self.id
                        != crate::ids::derived(
                            "unit.after_up",
                            &[body.run_id.as_bytes(), &body.position.to_be_bytes()],
                        )
                {
                    return Err(Error::Refused);
                }
                serde_json::to_value(plan).map_err(EngineError::from)?
            }
            _ => return Err(Error::Refused),
        };
        if self.body != expected {
            return Err(Error::Refused);
        }
        Ok(())
    }
    fn matches(&self, id: Id, machine: Id, kind: &str, serial: Option<&str>, body: &Json) -> bool {
        self.id == id
            && self.machine == machine
            && self.kind.as_str() == kind
            && self.serial.as_deref() == serial
            && self.body == *body
    }
}

/// A disjoint preparation-purpose carrier in the existing private command evidence channel.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEvidence {
    request: Preparation,
    key: HarnessKey,
    command: NativeCommand,
}
impl std::fmt::Debug for CommandEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdoptionPreparationCommand([private])")
    }
}
impl CommandEvidence {
    pub(crate) fn preparation(&self) -> &Preparation {
        &self.request
    }
    /// Factual owner fence before a canonical command row; the live verifier still grants nothing.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        lock_retained_source(conn, &self.request).await
    }
    /// The native builder, not an incoming operation claim, fixes this command kind.
    pub fn command_kind(&self) -> &'static str {
        self.command.kind.as_str()
    }
    async fn current(
        &self,
        conn: &mut Conn,
        command: CommandView<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if !self.command.matches(
            command.id,
            command.machine,
            command.kind,
            command.serial,
            command.body,
        ) {
            return Ok(None);
        }
        match self
            .request
            .checkpoint_for(conn, clock, self.key, Some(self.command.clone()))
            .await
        {
            Ok(check) => {
                let caller = check.caller.clone();
                let until = check.until;
                check.check(clock)?;
                Ok(Some(vec![(Some(caller), until)]))
            }
            Err(Error::Refused | Error::Binding(crate::operation_scope::BindingError::Refused)) => {
                Ok(None)
            }
            Err(error) => Err(engine(error)),
        }
    }
    /// Real proposed queue row, not a fabricated stored command or an admitted full act.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            CommandView {
                id: command.id,
                machine: command.machine_id,
                kind: command.kind,
                serial: command.serial_key,
                body: command.body,
            },
            clock,
        )
        .await
    }
    /// Re-authorize only actual unaccepted publication; accepted-result cleanup stays native.
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            CommandView {
                id: command.id,
                machine: command.machine_id,
                kind: &command.kind,
                serial: command.serial_key(),
                body: &command.body,
            },
            clock,
        )
        .await
    }
}
struct CommandView<'a> {
    id: Id,
    machine: Id,
    kind: &'a str,
    serial: Option<&'a str>,
    body: &'a Json,
}
