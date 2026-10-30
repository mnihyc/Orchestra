//! The credential's own harness as the facts, models and reset source, preserving native calls and failures.

use crate::harnesses::Process;
use async_trait::async_trait;
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::facts::normalize;
use orch_agent_contract::facts::{ProviderFacts, ResetRequest, ResetResult};
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_provider_contract::source::{SourceDescriptorFacts, SourceDescriptorModels};
use orch_provider_contract::{
    FactMember, FactsReport, FactsSource, ModelsReport, ModelsSource, ResetReport, SourceAsk,
    SourceDescriptor, SourceError, SourceErrorCode,
};
use orch_types::{Clock, Id, Json};
use std::sync::Arc;
use std::sync::{Mutex, PoisonError};

/// Reads the first signed-in harness that can answer; renewal stays a native credential operation.
pub async fn read(
    processes: &[Arc<Process>],
    renew: bool,
) -> Result<Option<ProviderFacts>, CommandError> {
    for process in processes {
        if renew {
            process.sign_in_allowed()?;
        }
        match process
            .call(move |adapter| {
                Box::pin(async move {
                    if renew {
                        adapter.renew_provider_credential().await
                    } else {
                        adapter.provider_facts().await
                    }
                })
            })
            .await
        {
            Err(error) if error.code == ErrorCode::LoginRequired.as_str() => {}
            answer => return answer.map(Some),
        }
    }
    Ok(None)
}

/// Lists a harness's offered models without accepting them or enabling any model parameter.
pub async fn list(processes: &[Arc<Process>]) -> Result<Vec<ModelSuggestion>, CommandError> {
    let mut refused = None;
    for process in processes {
        match process.call(|adapter| adapter.discover_models()).await {
            Err(error) if error.code == ErrorCode::LoginRequired.as_str() => refused = Some(error),
            answer => return answer,
        }
    }
    match refused {
        Some(error) => Err(error),
        None => Ok(Vec::new()),
    }
}

/// The reset id and idempotency key stay in the native source that listed the reset.
pub async fn spend(process: &Process, request: ResetRequest) -> Result<ResetResult, CommandError> {
    process
        .call(move |adapter| adapter.consume_reset(request))
        .await
}

/// The normalized members the harness actually told; typed default nulls are not fresh empty reports.
pub fn report(facts: &ProviderFacts) -> Result<FactsReport, SourceError> {
    let mut facts = facts.clone();
    normalize(&mut facts);
    let raw = serde_json::to_value(&facts).map_err(|_| {
        SourceError::new(
            SourceErrorCode::Malformed,
            "the harness's provider facts have another shape",
        )
    })?;
    let mut report = serde_json::Map::new();
    for member in FactMember::ALL {
        if (facts.was_told(member) || facts.holds(member))
            && let Some(value) = raw.get(member.as_str())
        {
            report.insert(member.as_str().to_owned(), value.clone());
        }
    }
    Ok(FactsReport(Json::Object(report)))
}

/// One command's native observation, shared by the credential keeper and the adapter facts source.
/// It never caches an observation into a later command.
pub struct Adapter {
    provider: Id,
    processes: Vec<Arc<Process>>,
    renew: bool,
    clock: Arc<dyn Clock>,
    observed: tokio::sync::OnceCell<Result<Option<ProviderFacts>, CommandError>>,
    latest: Mutex<Option<ProviderFacts>>,
}

impl Adapter {
    /// Uses only the harness processes of this provider and the connector's observation clock.
    #[must_use]
    pub fn new(
        provider: Id,
        processes: Vec<Arc<Process>>,
        renew: bool,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let processes = processes
            .into_iter()
            .filter(|process| process.provider_id == provider)
            .collect();
        Self {
            provider,
            processes,
            renew,
            clock,
            observed: tokio::sync::OnceCell::new(),
            latest: Mutex::new(None),
        }
    }

    /// Reads the keeper once, even where another source supplies the plan and spending.
    pub async fn observe(&self) -> Result<Option<ProviderFacts>, CommandError> {
        let answer = self
            .observed
            .get_or_init(|| async {
                let began = self.clock.time();
                let mut answer = read(&self.processes, self.renew).await?;
                if let Some(facts) = &mut answer
                    && facts.credential_verified == Some(true)
                    && facts
                        .field_times
                        .get("credential_verified")
                        .is_some_and(|at| *at < began)
                {
                    facts.credential_verified = None;
                    facts.field_times.remove("credential_verified");
                }
                Ok(answer)
            })
            .await
            .clone();
        if let Ok(Some(facts)) = &answer {
            *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(facts.clone());
        }
        answer
    }

    /// The native observation times, including an independently refused group's older time.
    #[must_use]
    pub fn latest(&self) -> Option<ProviderFacts> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn check(&self, ask: &SourceAsk) -> Result<(), SourceError> {
        if ask.provider.id == self.provider {
            Ok(())
        } else {
            Err(SourceError::new(
                SourceErrorCode::Refused,
                "the source belongs to another provider",
            ))
        }
    }

    fn descriptor(&self) -> SourceDescriptor {
        let native = self
            .processes
            .iter()
            .find_map(|process| process.started().1);
        let title = native
            .as_ref()
            .map_or("Harness", |native| native.harness.name.as_str());
        let mut descriptor = SourceDescriptor::new(
            "adapter",
            title,
            "Reads the sign-in through the harness that keeps it.",
        );
        let mut reports = native.as_ref().map_or_else(Vec::new, |native| {
            FactMember::of_kinds(&native.capabilities.provider_facts)
        });
        for member in [FactMember::Identity, FactMember::Credential] {
            if !reports.contains(&member) {
                reports.push(member);
            }
        }
        descriptor.facts = Some(SourceDescriptorFacts {
            spends_resets: reports.contains(&FactMember::AvailableResets),
            reports,
            plans: native
                .as_ref()
                .and_then(|native| native.native_provider.as_ref())
                .map(|native| native.plans.clone())
                .unwrap_or_default(),
            ..SourceDescriptorFacts::default()
        });
        if native
            .as_ref()
            .is_some_and(|native| native.capabilities.model_discovery)
        {
            descriptor.models = Some(SourceDescriptorModels::default());
        }
        descriptor
    }
}

/// Converts only the safe command sentence; dispatch scrubs every opened source and provider secret again.
pub(crate) fn command_error(error: CommandError) -> SourceError {
    let code = match error.code.as_str() {
        "unsupported" => SourceErrorCode::Unsupported,
        "facts_not_told" | "not_told" => SourceErrorCode::NotTold,
        "login_required" => SourceErrorCode::Refused,
        "timeout" | "expired" => SourceErrorCode::Timeout,
        _ => SourceErrorCode::Unavailable,
    };
    SourceError {
        code,
        message: error.detail,
        retryable: error.retryable || code.retryable(),
    }
}

#[async_trait]
impl FactsSource for Adapter {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }
    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        self.check(&ask)?;
        let facts = self
            .observe()
            .await
            .map_err(command_error)?
            .ok_or_else(|| {
                SourceError::new(
                    SourceErrorCode::NotTold,
                    "the harness has no current sign-in",
                )
            })?;
        report(&facts)
    }
    async fn spend_reset(
        &self,
        ask: SourceAsk,
        request: ResetRequest,
    ) -> Result<ResetReport, SourceError> {
        self.check(&ask)?;
        if !self.descriptor().spends_resets() {
            return Err(SourceError::unsupported("resets.spend"));
        }
        let process = self.processes.first().ok_or_else(|| {
            SourceError::new(
                SourceErrorCode::NotTold,
                "the harness has no current sign-in",
            )
        })?;
        let result = spend(process, request).await.map_err(command_error)?;
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(result.facts.clone());
        Ok(ResetReport {
            outcome: result.outcome,
            facts: Some(report(&result.facts)?),
        })
    }
}

#[async_trait]
impl ModelsSource for Adapter {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor()
    }
    async fn list_models(&self, ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        self.check(&ask)?;
        if !self.descriptor().lists_models() {
            return Ok(ModelsReport::unlisted());
        }
        Ok(ModelsReport::listed(
            &list(&self.processes).await.map_err(command_error)?,
        ))
    }
}
