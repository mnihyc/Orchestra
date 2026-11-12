//! One native discovery/history observation and the effect-free plan that follows it.

use super::*;
use orch_db::queries::environments::thread_engine::EnvironmentRow;

/// Once-only native observation. It is not serialized authority or a replacement supplier.
pub struct Fetched {
    pub(super) adoption: Adoption,
    pub(super) session: Held,
    pub(super) machine: Id,
    pub(super) recorded: Recorded,
    preparation: Option<operation::Preparation>,
    pub(super) record_source: Option<operation::RecordObservation>,
    pub(super) entries: Option<Result<Vec<RecordedEntry>, String>>,
}
impl Fetched {
    /// The actual request that produced this observation.
    pub fn adoption(&self) -> &Adoption {
        &self.adoption
    }
    /// Actual captured original operation from the Registry's initial preparation.
    pub fn preparation(&self) -> Option<&operation::Preparation> {
        self.preparation.as_ref()
    }
}

/// Raw current selected operands. Rendering remains outside the retryable selector.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    environment: Option<EnvironmentRow>,
    directory: Option<String>,
    register_first: Option<String>,
    adopted_thread_id: Option<Id>,
    reported: Option<String>,
    model: Option<String>,
    model_reason: Option<AdoptModelReason>,
    turns: i64,
    history_missing: Option<AdoptHistoryMissing>,
}
impl Planned {
    /// Only actual existing identities; a missing Environment is never invented.
    pub fn environment(&self) -> Option<&EnvironmentRow> {
        self.environment.as_ref()
    }
    /// Whether the native model fallback actually consulted this Topic's policy.
    pub fn reads_topic_policy(&self) -> bool {
        self.reported
            .as_deref()
            .is_some_and(|model| !model.trim().is_empty())
    }
    /// An actual existing prior adoption, not a newly created Thread.
    pub fn adopted_thread(&self) -> Option<Id> {
        self.adopted_thread_id
    }
}

impl Engine {
    /// The original native supplier once, with an exact preparation-purpose source at each setup/IO seam.
    pub async fn fetch_adoption(
        &self,
        caller: &Caller,
        credential: operation::HttpCredential,
        adoption: Adoption,
        transport: Option<std::sync::Arc<dyn operation::Transport>>,
        until: Option<Time>,
    ) -> Result<Fetched, AdoptError> {
        self.may_adopt(caller, &adoption).await?;
        let preparation = operation::Preparation::capture(caller, credential, adoption.clone())?
            .with_transport(transport, until);
        let (held, machine) = self
            .held_session_with_preparation(&adoption, Some(&preparation))
            .await?;
        let mut record_source = if preparation.origin().operation() == "ThreadAdopt" {
            Some(
                operation::RecordObservation::<operation::Adopting>::open(
                    self,
                    &preparation,
                    crate::harnesses::HarnessKey {
                        provider: adoption.provider,
                        agent: adoption.agent,
                        machine,
                    },
                )
                .await?,
            )
        } else {
            None
        };
        let record_preparation = record_source
            .as_ref()
            .map(|source| source.preparation().clone())
            .unwrap_or_else(|| preparation.clone());
        let recorded = self
            .read_session_record_observed::<operation::Adopting>(
                machine,
                &adoption,
                preparation.origin().operation() == "ThreadAdopt",
                Some(&record_preparation),
                record_source.as_mut(),
            )
            .await?;
        if let Some(source) = &mut record_source {
            let _ = source.refresh(self).await?;
        }
        let entries = match (&recorded, record_source.as_mut()) {
            (Recorded::Read(read), Some(source)) => {
                let result = self.record_entries_from(read, Some(source)).await;
                let _ = source.refresh(self).await?;
                Some(match result {
                    Ok(entries) => Ok(entries),
                    Err(RecordReadError::Content(why)) => Err(why),
                    Err(RecordReadError::Source(error)) => return Err(error.into()),
                })
            }
            _ => None,
        };
        Ok(Fetched {
            adoption,
            session: held,
            machine,
            recorded,
            preparation: Some(preparation.without_transport()),
            record_source,
            entries,
        })
    }

    /// An actual native caller supplies its own authority; no HTTP Session is fabricated.
    pub(super) async fn fetch_native_for<E: operation::Entry>(
        &self,
        caller: &Caller,
        adoption: Adoption,
    ) -> Result<Fetched, AdoptError> {
        if caller
            .admitted_operation()
            .is_some_and(|operation| operation != E::DECLARATION.operation)
        {
            return Err(CreateThreadError::OperationScope.into());
        }
        let mut tx = self.inner.db.begin().await?;
        if orch_db::queries::transaction_guard::lock_native_actor(&mut tx, caller.user())
            .await?
            .is_none()
        {
            return Err(CreateThreadError::OperationScope.into());
        }
        let restored = orch_authz::CapturedCaller::capture(caller)
            .restore_current(&mut tx, self.inner.clock.as_ref())
            .await
            .map_err(EngineError::from)?
            .ok_or(CreateThreadError::OperationScope)?;
        let facts = operation::initial_facts(&mut tx, &adoption).await?;
        let plan = crate::operation_scope::sources::bind_known(E::ROUTE, E::DECLARATION, &facts)
            .map_err(EngineError::unreadable)?;
        let accepted = plan
            .preparation_caller(&restored)
            .map_err(operation::scope_for_place)?;
        let principal =
            Principal::load(&mut tx, accepted.clone(), Time::new(self.inner.clock.now()))
                .await
                .map_err(EngineError::from)?;
        operation::require_initial(&mut tx, &principal, &adoption).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.fetch_adoption(
            &accepted,
            operation::HttpCredential::Native,
            adoption,
            None,
            None,
        )
        .await
    }

    /// Existing native selection and model fallback, with no RPC, rendering or business mutation.
    pub async fn prepare_adoption_plan_in(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        fetched: &Fetched,
    ) -> Result<Planned, AdoptError> {
        let adoption = &fetched.adoption;
        let adopted_thread_id =
            thread_rows::adopting_threads(conn, std::slice::from_ref(&adoption.native_session_ref))
                .await?
                .into_iter()
                .next()
                .map(|(_, thread)| thread);
        let place = self
            .adopted_place(
                conn,
                principal,
                adoption,
                fetched.machine,
                fetched.session.cwd.as_deref(),
                "ThreadAdoptPreview",
            )
            .await?;
        let (environment, directory, register_first) = match place {
            Place::In {
                environment,
                directory,
            } => (
                Some(
                    environments::environment(conn, environment)
                        .await?
                        .ok_or(CreateThreadError::EnvironmentUnavailable)?,
                ),
                Some(directory),
                None,
            ),
            Place::RegisterFirst(directory) => (None, None, directory),
        };
        let (reported, effort, turns, history_missing) = match &fetched.recorded {
            Recorded::Read(read) => (read.model.clone(), read.effort.clone(), read.turns, None),
            Recorded::Unsupported(_) => (None, None, 0, Some(AdoptHistoryMissing::Unsupported)),
            Recorded::Failed(why) => {
                tracing::info!(%why,"what a session recorded could not be read for its plan");
                (None, None, 0, Some(AdoptHistoryMissing::Unreadable))
            }
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
        Ok(Planned {
            environment,
            directory,
            register_first,
            adopted_thread_id,
            reported,
            model: choice.model,
            model_reason: choice.reason,
            turns,
            history_missing,
        })
    }

    /// The native plan body, after its actual selected sources and ordinary reading have been admitted.
    pub async fn render_adoption_plan_in(
        &self,
        conn: &mut Conn,
        fetched: &Fetched,
        planned: &Planned,
    ) -> Result<AdoptPlan, AdoptError> {
        self.render_adoption_plan_with_names_in(conn, fetched, planned, None)
            .await
    }

    /// Keep the same native body and reference renderer, with factual optional name bounds.
    pub async fn render_adoption_plan_with_names_in(
        &self,
        conn: &mut Conn,
        fetched: &Fetched,
        planned: &Planned,
        names: Option<&std::collections::BTreeSet<Id>>,
    ) -> Result<AdoptPlan, AdoptError> {
        let environment = match &planned.environment {
            Some(row) => Some(
                serde_json::from_value(
                    crate::views::environment_ref_json_with_names(
                        conn,
                        row,
                        Time::new(self.inner.clock.now()),
                        names,
                    )
                    .await?,
                )
                .map_err(EngineError::from)?,
            ),
            None => None,
        };
        Ok(AdoptPlan {
            agent_access: None,
            session: orch_api_contract::resources::NativeSession {
                native_session_ref: fetched.session.native_session_ref.clone(),
                title: fetched.session.title.clone(),
                cwd: fetched.session.cwd.clone(),
                updated_at: fetched.session.updated_at,
                adopted_thread_id: planned.adopted_thread_id,
            },
            name: thread_name(
                fetched.adoption.name.as_deref(),
                fetched.session.title.as_deref(),
            )
            .0,
            environment,
            directory: planned.directory.clone(),
            register_first: planned.register_first.clone(),
            session_model: planned.reported.clone(),
            model: planned.model.clone(),
            model_reason: planned.model_reason,
            turns: planned.turns,
            history_missing: planned.history_missing,
            adopted_thread_id: planned.adopted_thread_id,
        })
    }
}
