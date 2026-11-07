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
}
impl Fetched {
    pub(super) fn ordinary(
        adoption: Adoption,
        session: Held,
        machine: Id,
        recorded: Recorded,
    ) -> Self {
        Self {
            adoption,
            session,
            machine,
            recorded,
            preparation: None,
        }
    }
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
        transport: std::sync::Arc<dyn operation::Transport>,
        until: Option<Time>,
    ) -> Result<Fetched, AdoptError> {
        self.may_adopt(caller, &adoption).await?;
        let preparation = operation::Preparation::capture(caller, credential, adoption.clone())?
            .with_transport(transport, until);
        let (held, machine) = self
            .held_session_with_preparation(&adoption, Some(&preparation))
            .await?;
        let recorded = self
            .read_session_record_with_preparation(
                machine,
                &adoption,
                preparation.origin().operation() == "ThreadAdopt",
                Some(&preparation),
            )
            .await?;
        Ok(Fetched {
            adoption,
            session: held,
            machine,
            recorded,
            preparation: Some(preparation.without_transport()),
        })
    }

    /// Existing native selection and model fallback, with no RPC, rendering or business mutation.
    pub async fn prepare_adoption_plan_in(
        &self,
        conn: &mut Conn,
        caller: &Caller,
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
                caller,
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
                caller.user(),
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
