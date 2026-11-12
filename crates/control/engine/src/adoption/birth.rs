//! The existing native adoption, with one rollbackable creator/history transaction.

use super::*;
use crate::binding::BirthAgentAccess;
use orch_api_contract::routes::Endpoint;
use orch_types::Clock;
use std::sync::Arc;

/// Actual native operands of this request, never a reusable admission or serialized job.
pub struct PreparedAdoption {
    actor: Id,
    transaction: i64,
    principal: Principal,
    new: NewThread,
    birth: Birth,
    fetched: Arc<Fetched>,
    model_reason: Option<AdoptModelReason>,
    reported: Option<String>,
    gift_project: Option<Id>,
}
impl PreparedAdoption {
    /// The current native actor, including the supplied request's holds.
    pub fn principal(&self) -> &Principal {
        &self.principal
    }
    /// The actual selected binding and name, without allocating a fictional Thread.
    pub fn new_thread(&self) -> &NewThread {
        &self.new
    }
    /// The actual native birth choice; a creator can narrow a preset before application.
    pub fn birth(&self) -> &Birth {
        &self.birth
    }
    /// The immutable actual native request that produced these operands.
    pub fn request(&self) -> &Adoption {
        &self.fetched.adoption
    }
    /// Actual retained record's raw lifetime minimum, independent of browser-session presence.
    pub fn record_until(&self) -> Option<Time> {
        self.fetched
            .record_source
            .as_ref()
            .and_then(|source| source.until())
    }
    /// The session name supplied by the actual native observation.
    pub fn native_session_ref(&self) -> &str {
        &self.fetched.adoption.native_session_ref
    }
    /// Factual source of the creator's scope-adjusted gift intent, frozen under its Topic lock.
    pub fn gift_source(
        &self,
        choice: &BirthAgentAccess,
        thread: Id,
    ) -> Result<Option<ObjectRef>, EngineError> {
        let choice = match choice {
            BirthAgentAccess::Exact(choice) | BirthAgentAccess::FrozenPreset(choice) => choice,
            BirthAgentAccess::ResolvePreset => {
                return Err(EngineError::unreadable(
                    "the native adoption preset was not frozen",
                ));
            }
        };
        if choice.parts == orch_api_contract::share::ShareParts::default()
            || choice.where_ == orch_api_contract::nodes::AgentAccessWhere::Nothing
        {
            return Ok(None);
        }
        use orch_api_contract::nodes::AgentAccessWhere;
        Ok(Some(match choice.where_ {
            AgentAccessWhere::Thread => ObjectRef {
                kind: ObjectKind::Thread,
                id: thread,
            },
            AgentAccessWhere::Topic => ObjectRef {
                kind: ObjectKind::Topic,
                id: self.new.topic,
            },
            AgentAccessWhere::Project => ObjectRef {
                kind: ObjectKind::Project,
                id: self.gift_project.ok_or_else(|| {
                    EngineError::unreadable(
                        "the native adoption has no selected Project gift source",
                    )
                })?,
            },
            AgentAccessWhere::Nothing => return Ok(None),
        }))
    }
}

/// Generated outcome of the accepted native act, without historical content or source names.
#[derive(Debug, Clone)]
pub struct Adopted {
    /// The actual created Thread/segment and its final native sequence.
    pub created: CreatedThread,
    /// Native newly imported items, excluding duplicate references.
    pub history_items: i64,
}
/// A private native commit handle; its follow-up work belongs to the same transaction.
pub struct AdoptionWrite {
    outcome: Adopted,
    finished: crate::unit::Finished,
}
impl AdoptionWrite {
    /// Genuine generated outcome, before the caller's final guarded commit.
    pub fn outcome(&self) -> &Adopted {
        &self.outcome
    }
}

async fn actor_in(
    conn: &mut Conn,
    original: &Principal,
    fetched: &Fetched,
    clock: &dyn Clock,
) -> Result<Principal, AdoptError> {
    let adoption = &fetched.adoption;
    if original
        .caller()
        .admitted_operation()
        .is_some_and(|operation| operation != "ThreadAdopt")
    {
        return Err(CreateThreadError::OperationScope.into());
    }
    let caller = orch_authz::CapturedCaller::capture(original.caller())
        .restore_current(conn, clock)
        .await
        .map_err(EngineError::from)?;
    let Some(mut caller) = caller else {
        return Err(CreateThreadError::OperationScope.into());
    };
    if let Some(source) = fetched.record_source.as_ref() {
        let check = source.check_in(conn, &caller, clock).await?;
        caller = caller
            .intersect_restrictions(check.caller())
            .map_err(EngineError::unreadable)?;
        check.check(clock)?;
    }
    let facts = operation::initial_facts(conn, adoption).await?;
    let plan = crate::operation_scope::sources::bind_known(
        orch_api_contract::nodes::ThreadAdopt::ROUTE,
        &orch_api_contract::adoption_sources::ADOPT_PREPARATION,
        &facts,
    )
    .map_err(EngineError::unreadable)?;
    let caller = plan
        .preparation_caller(&caller)
        .map_err(operation::scope_for_place)?;
    let principal = Principal::load(conn, caller, clock.time())
        .await
        .map_err(EngineError::from)?;
    let principal = match original.second_step_hold() {
        Some(hold) => principal.held_for_second_step(hold.clone()),
        None => principal,
    };
    let user = orch_db::queries::identity::user_by_id(conn, principal.user())
        .await?
        .ok_or(AdoptError::Gone)?;
    let session = fetched
        .preparation()
        .and_then(|preparation| preparation.browser_session());
    Ok(
        match crate::operation_scope::account_policy::session_hold(conn, &user, session).await? {
            Some(hold) => principal.held_for_second_step(hold),
            None => principal,
        },
    )
}

impl Engine {
    /// Select the real native binding/model after its source waits, on the creator transaction.
    /// No source, descriptor, model or filesystem IO occurs inside this factory.
    pub async fn prepare_adoption_birth_in(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        fetched: Arc<Fetched>,
    ) -> Result<PreparedAdoption, AdoptError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(
                EngineError::unreadable("native adoption birth requires its transaction").into(),
            );
        }
        if fetched
            .preparation()
            .is_some_and(|source| source.origin().operation() != "ThreadAdopt")
        {
            return Err(CreateThreadError::OperationScope.into());
        }
        if orch_db::queries::transaction_guard::lock_native_actor(conn, principal.user())
            .await?
            .is_none()
        {
            return Err(AdoptError::Gone);
        }
        let adoption = &fetched.adoption;
        operation::require_initial(conn, principal, adoption).await?;
        let key = crate::harnesses::HarnessKey {
            provider: adoption.provider,
            agent: adoption.agent,
            machine: fetched.machine,
        };
        if !operation::lock_sources(conn, adoption.topic, key).await? {
            return Err(AdoptError::Gone);
        }
        adoption::lock_session(conn, &adoption.native_session_ref).await?;
        if let Some((_, thread)) =
            thread_rows::adopting_threads(conn, std::slice::from_ref(&adoption.native_session_ref))
                .await?
                .into_iter()
                .next()
        {
            return Err(AdoptError::AlreadyAdopted { thread });
        }
        if let Some(source) = &fetched.record_source {
            source
                .check_in(conn, principal.caller(), self.inner.clock.as_ref())
                .await?
                .check(self.inner.clock.as_ref())?;
        }
        let principal = actor_in(conn, principal, &fetched, self.inner.clock.as_ref()).await?;
        operation::require_initial(conn, &principal, adoption).await?;
        let (environment, directory) = match self
            .adopted_place(
                conn,
                &principal,
                adoption,
                fetched.machine,
                fetched.session.cwd.as_deref(),
                "ThreadAdopt",
            )
            .await?
        {
            Place::In {
                environment,
                directory,
            } => (environment, directory),
            Place::RegisterFirst(directory) => return Err(AdoptError::NoEnvironment { directory }),
        };
        let (reported, effort) = match &fetched.recorded {
            Recorded::Read(read) => (read.model.clone(), read.effort.clone()),
            _ => (None, None),
        };
        let choice = self
            .adopted_model(
                conn,
                principal.user(),
                adoption,
                reported.as_deref(),
                effort.as_deref(),
            )
            .await?;
        let (name, titled) =
            thread_name(adoption.name.as_deref(), fetched.session.title.as_deref());
        let mut birth = Birth {
            agent_access: adoption.agent_access.into(),
            prepare_source: Some(false),
            titled_by_automation: titled,
            ..Birth::default()
        };
        birth.agent_access = birth
            .agent_access
            .freeze(conn, adoption.topic)
            .await
            .map_err(|error| CreateThreadError::AgentAccess(Box::new(error)))?;
        let choice_ref = match &birth.agent_access {
            BirthAgentAccess::Exact(choice) | BirthAgentAccess::FrozenPreset(choice) => choice,
            BirthAgentAccess::ResolvePreset => {
                return Err(EngineError::unreadable("the adoption preset was not frozen").into());
            }
        };
        let gift_project = if choice_ref.where_
            == orch_api_contract::nodes::AgentAccessWhere::Project
            && choice_ref.parts != orch_api_contract::share::ShareParts::default()
        {
            let rows = orch_db::queries::hierarchy::with_ancestors(conn, &[adoption.topic]).await?;
            let topic = rows
                .iter()
                .find(|row| row.id == adoption.topic)
                .ok_or(AdoptError::Gone)?;
            Some(
                topic
                    .path
                    .iter()
                    .rev()
                    .find_map(|id| {
                        rows.iter()
                            .find(|row| row.id == *id && row.kind == "project")
                            .map(|row| row.id)
                    })
                    .ok_or(AdoptError::Gone)?,
            )
        } else {
            None
        };
        let transaction = sqlx::query_scalar("SELECT txid_current()")
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        let new = NewThread {
            name,
            topic: adoption.topic,
            agent: adoption.agent,
            provider: adoption.provider,
            environment,
            directory,
            arrangement: None,
            model: choice.model,
            effort: choice.effort,
        };
        Ok(PreparedAdoption {
            actor: principal.user(),
            transaction,
            principal,
            new,
            birth,
            fetched,
            model_reason: choice.reason,
            reported,
            gift_project,
        })
    }

    /// The same native marker/import/report writer after the complete creator gate.
    pub async fn write_adoption_in<'a>(
        &self,
        conn: &'a mut Conn,
        principal: &Principal,
        prepared: &PreparedAdoption,
        created: &CreatedThread,
        before: impl FnOnce() -> Result<(), EngineError> + Send + 'a,
    ) -> Result<AdoptionWrite, AdoptError> {
        let transaction: i64 = sqlx::query_scalar("SELECT txid_current()")
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        if transaction != prepared.transaction || principal.user() != prepared.actor {
            return Err(EngineError::unreadable(
                "native adoption operands changed their transaction or actor",
            )
            .into());
        }
        let segment = orch_db::queries::threads::segment(conn, created.segment)
            .await?
            .ok_or(AdoptError::Gone)?;
        if segment.thread_id != created.thread
            || segment.provider_id != prepared.new.provider
            || segment.agent_id != prepared.new.agent
            || segment.environment_id != prepared.new.environment
            || segment.native_session_ref.as_deref() != Some(prepared.native_session_ref())
        {
            return Err(
                EngineError::unreadable("the created segment has another native adoption").into(),
            );
        }
        let (turns, items, missing, history_error, report, entries) =
            match &prepared.fetched.recorded {
                Recorded::Read(read) => match prepared.fetched.entries.as_ref() {
                    Some(Ok(entries)) => (
                        read.turns,
                        i64::try_from(entries.len()).unwrap_or(i64::MAX),
                        None,
                        None,
                        Some(serde_json::to_value(&read.report).map_err(EngineError::from)?),
                        Some(entries),
                    ),
                    Some(Err(why)) => (
                        0,
                        0,
                        Some(AdoptHistoryMissing::Unreadable),
                        Some(why.clone()),
                        None,
                        None,
                    ),
                    None => {
                        return Err(EngineError::unreadable(
                            "the native adoption record has not been prepared",
                        )
                        .into());
                    }
                },
                Recorded::Unsupported(why) => (
                    0,
                    0,
                    Some(AdoptHistoryMissing::Unsupported),
                    Some(why.clone()),
                    None,
                    None,
                ),
                Recorded::Failed(why) => (
                    0,
                    0,
                    Some(AdoptHistoryMissing::Unreadable),
                    Some(why.clone()),
                    None,
                    None,
                ),
            };
        let author = adoption::machine_owner(conn, prepared.fetched.machine).await?;
        let mut unit = Unit::with_first_effect(conn, self.inner.clock.now(), before);
        let marker = json!({"marker":"adopted","data":{"by":crate::views::user_ref_json(unit.conn,prepared.actor).await?,"native_session_ref":prepared.native_session_ref(),"turns":turns,"items":items,"history_missing":missing,"history_error":history_error,"session_model":prepared.reported,"model_reason":prepared.model_reason,"history_author":author,"history_before":prepared.fetched.session.updated_at.unwrap_or_else(||Time::new(unit.now()))}});
        ingest::add_marker(
            &mut unit,
            created.thread,
            created.segment,
            None,
            &marker,
            "adopted",
        )
        .await?;
        let written = match entries {
            Some(entries) => {
                self.write_session_record(
                    &mut unit,
                    created.thread,
                    created.segment,
                    author,
                    entries,
                )
                .await?
            }
            None => Written::default(),
        };
        if let Some(report) = &report {
            adoption::set_adopted_report(unit.conn, created.segment, report).await?;
        }
        let mut created = created.clone();
        created.seq = unit.seq_given(created.thread).unwrap_or(created.seq);
        let finished = unit.finish().await?;
        Ok(AdoptionWrite {
            outcome: Adopted {
                created,
                history_items: written.items,
            },
            finished,
        })
    }

    /// Commit the native creator/history together and preserve its ordinary follow-up work.
    pub async fn commit_adoption(
        &self,
        tx: orch_db::Tx,
        write: AdoptionWrite,
    ) -> Result<Adopted, EngineError> {
        self.commit(tx, write.finished).await?;
        Ok(write.outcome)
    }
}
