//! One fenced native claim; retries come only from the ordinary durable call rules.
use super::*;
use orch_plugin_contract::refs::NodeRef;
use orch_provider_contract::{
    SourceErrorCode,
    calls::Purpose,
    capabilities::{CallSource, CapabilityCall, Executors},
};
use orch_summarizer_contract::{descriptor::SummaryField, summarizer::SummaryRequest};
use serde_json::json;
use std::sync::Arc;

impl Engine {
    /// Claims a bounded batch on short transactions, then releases every connection before model IO.
    pub async fn summarize_resources(&self) -> Result<(), EngineError> {
        self.queue_automatic_summaries().await?;
        let now = self.inner.clock.now();
        // Only a real call-rule continuation can revive the same failed logical request.
        // A later Refresh supersedes it; neither a poll nor a worker crash invents a paid retry.
        let mut tx = self.db().begin().await?;
        rows::revive(&mut tx, now, self.config().sweep_batch).await?;
        tx.commit().await.map_err(DbError::from)?;
        let mut conn = self.db().acquire().await?;
        let until = now
            + chrono::Duration::seconds(
                i64::from(
                    orch_link_contract::commands::CommandKind::ProviderComplete.deadline_secs(),
                ) + super::super::CLAIM_GRACE_SECS,
            );
        let mut claimed = Vec::new();
        for _ in 0..super::super::SUMMARY_BATCH {
            match rows::claim(&mut conn, now, until).await? {
                Some(row) => claimed.push(row),
                None => break,
            }
        }
        drop(conn);
        for request in claimed {
            let engine = self.clone();
            tokio::spawn(async move {
                if let Err(error) = engine.run_resource_summary(request).await {
                    tracing::warn!(error_kind=?std::mem::discriminant(&error),"a resource summary could not settle; its original work remains recoverable");
                }
            });
        }
        Ok(())
    }

    async fn run_resource_summary(&self, request: rows::Request) -> Result<(), EngineError> {
        let token = request.lease_token.ok_or_else(refused)?;
        let mut told = None;
        let preparation = self.prepare_resource_work(&request, token, &mut told).await;
        let work = match preparation {
            Ok(work) => work,
            Err(error) => {
                self.finish_resource_failure(&request, token, None, told, &error)
                    .await?;
                return Ok(());
            }
        };
        let attempt = self.execute_resource_work(&work, &mut told).await;
        match attempt {
            Ok((text, actual)) => {
                let current = work.candidate(&actual);
                let mut tx = self.db().begin().await?;
                let frame = match current.frame(&mut tx, self.inner.clock.as_ref()).await {
                    Ok(frame) => frame,
                    Err(error) => {
                        drop(tx);
                        self.finish_resource_discarded(&request, token, &error)
                            .await?;
                        return Ok(());
                    }
                };
                let parts = frame.into_parts();
                let producer = json!({"kind":"model","provider_id":actual.provider_id,"model_name":actual.model_name});
                let refs = serde_json::to_value(&work.refs)?;
                parts
                    .first_effect
                    .check(self.inner.clock.as_ref())
                    .map_err(command::frame_error)?;
                rows::finish(
                    &mut tx,
                    request.id,
                    token,
                    self.inner.clock.as_ref(),
                    rows::Outcome::Paragraph(rows::Paragraph {
                        text: &text,
                        producer: &producer,
                        source_snapshot: &work.source,
                        source_refs: &refs,
                    }),
                )
                .await?;
                crate::calls::finish_service(
                    &mut tx,
                    parts.value.material.identity.place(),
                    Purpose::Summary,
                    request.id,
                )
                .await?;
                tx.commit().await.map_err(DbError::from)?;
            }
            Err(error) => {
                self.finish_resource_failure(&request, token, Some(&work), told, &error)
                    .await?;
            }
        }
        Ok(())
    }

    /// The request ends without a paragraph, and says why: the first thing the person can change
    /// that no longer holds. A request that was refused before any model was asked, and for which
    /// nothing can be named, found its source or settings changed under it: it is discarded, as a
    /// newer act supersedes it, and no failure is shown. Once the model was asked, a request with no
    /// nameable cause still fails, so that the person is told that something went wrong.
    async fn finish_resource_failure(
        &self,
        request: &rows::Request,
        token: Id,
        work: Option<&Work>,
        told: Option<Reason>,
        error: &EngineError,
    ) -> Result<(), EngineError> {
        // What no longer holds is the reason when something does not; what the work itself saw of
        // the model is the reason when everything else still holds. A source with nothing in it needs
        // no model, whatever the model's state.
        let judged = if told == Some(Reason::NothingNew) {
            None
        } else {
            match self.explain_resource_failure(request, work).await {
                Ok(reason) => reason,
                Err(cause) => {
                    tracing::warn!(request = %request.id, error = %cause, "the reason a summary failed could not be looked up");
                    None
                }
            }
        };
        let reason = judged
            .or(told)
            .or_else(|| work.is_some().then_some(Reason::Unexpected));
        let problem = reason.map(|reason| {
            json!({"code":"summary_unavailable","message":reason.sentence(),"reason":reason.code()})
        });
        match reason {
            Some(reason) => tracing::warn!(
                request = %request.id,
                reason = reason.code(),
                error = %error,
                "a summary could not be written: {}",
                reason.sentence()
            ),
            None => tracing::info!(
                request = %request.id,
                error = %error,
                "a summary request was discarded: its source or settings changed after it was asked"
            ),
        }
        let mut tx = self.db().begin().await?;
        rows::finish(
            &mut tx,
            request.id,
            token,
            self.inner.clock.as_ref(),
            match &problem {
                Some(problem) => rows::Outcome::Failed(problem),
                None => rows::Outcome::Discarded,
            },
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    /// A paragraph that was written about material that changed meanwhile is not kept; the next
    /// request is the person's, or the settings', to make.
    async fn finish_resource_discarded(
        &self,
        request: &rows::Request,
        token: Id,
        error: &EngineError,
    ) -> Result<(), EngineError> {
        tracing::info!(
            request = %request.id,
            error = %error,
            "a summary was discarded: its source or settings changed while it was written"
        );
        let mut tx = self.db().begin().await?;
        rows::finish(
            &mut tx,
            request.id,
            token,
            self.inner.clock.as_ref(),
            rows::Outcome::Discarded,
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    /// Who asked, as the work would restore them: the raw sign-in or key, the captured operation,
    /// or the standing of whoever pays for an automatic refresh. A caller that ended is a refusal.
    async fn asker_of(
        &self,
        conn: &mut Conn,
        request: &rows::Request,
        work: Option<&Work>,
    ) -> Result<Caller, EngineError> {
        if let Some(work) = work {
            return work.caller(conn, self.inner.clock.as_ref()).await;
        }
        let payer = request.paying_user.ok_or_else(refused)?;
        let http = request
            .accepted_source
            .get("http")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value::<operation_scope::http_origin::HttpOrigin>)
            .transpose()?;
        if let Some(http) = &http {
            return Ok(http
                .current(conn, self.inner.clock.as_ref())
                .await
                .map_err(command::frame_error)?
                .caller()
                .clone());
        }
        if let Some(origin) = &request.accepted_operation {
            let origin: CapturedOperation = serde_json::from_value(origin.clone())?;
            if origin.user() != payer
                || origin.operation() != summary_sources::RESOURCE_REFRESH.operation
            {
                return Err(refused());
            }
            return origin
                .restore_current(conn, self.inner.clock.as_ref())
                .await?
                .ok_or_else(refused);
        }
        Ok(Caller::session(payer).with_admin_mode(false))
    }

    /// The first thing that no longer holds, looked at again in a transaction that is never
    /// committed: who asked, what the summary is about, the model and its provider, and the usage
    /// limits. `None` when everything still holds.
    async fn explain_resource_failure(
        &self,
        request: &rows::Request,
        work: Option<&Work>,
    ) -> Result<Option<Reason>, EngineError> {
        let mut tx = self.db().begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let caller = match self.asker_of(&mut tx, request, work).await {
            Ok(caller) => caller,
            Err(EngineError::BeforeEffect(_)) => return Ok(Some(Reason::AskerEnded)),
            Err(error) => return Err(error),
        };
        let principal = match Principal::load(&mut tx, caller, self.inner.clock.time()).await {
            Ok(principal) => principal,
            Err(orch_authz::Error::Db(error)) => return Err(error.into()),
            Err(_) => return Ok(Some(Reason::AskerEnded)),
        };
        let Some(target) = request
            .accepted_source
            .get("target")
            .cloned()
            .and_then(|target| serde_json::from_value::<SummaryTarget>(target).ok())
        else {
            return Ok(None);
        };
        let identity = match sources::identify(&mut tx, target).await {
            Ok(identity) => identity,
            Err(
                sources::SourceError::Unavailable
                | sources::SourceError::Authz(orch_authz::Error::NotFound(_)),
            ) => return Ok(Some(Reason::SourceGone)),
            Err(error) => return Err(source_error(error)),
        };
        let wanted: Vec<SummaryTarget> = match work {
            Some(work) => work.refs.clone(),
            None => vec![target],
        };
        for source in wanted {
            match sources::readable(&mut tx, &principal, source).await {
                Ok(true) => {}
                Ok(false) => return Ok(Some(Reason::SourceUnreadable)),
                Err(sources::SourceError::Unavailable) => return Ok(Some(Reason::SourceGone)),
                Err(error) => return Err(source_error(error)),
            }
        }
        let config = match work {
            Some(work) => work.config.clone(),
            None => {
                let chosen = request
                    .accepted_source
                    .get("model")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .map(serde_json::from_value::<SummaryModelChoice>)
                    .transpose()?;
                let policy = options::resolve(&mut tx, &identity).await?;
                let producer = policy
                    .producer
                    .as_ref()
                    .filter(|producer| producer.kind == "model");
                match (chosen, producer) {
                    (Some(choice), _) => {
                        let mut config: Config = serde_json::from_value(
                            json!({"provider_id":choice.provider_id,"model_name":choice.model}),
                        )?;
                        config.params = choice.params.clone().unwrap_or_else(|| json!({}));
                        config
                    }
                    (None, Some(producer)) => match Config::read(&producer.config) {
                        Ok(config) => config,
                        Err(_) => return Ok(Some(Reason::NoSummarizer)),
                    },
                    (None, None) => return Ok(Some(Reason::NoSummarizer)),
                }
            }
        };
        if let Err(reason) = judge_model(&mut tx, &principal, identity.place(), &config).await? {
            return Ok(Some(reason));
        }
        let protocol = match resources::provider(&mut tx, config.provider_id).await? {
            Some(provider) => completion_protocol(&provider.protocols)?,
            None => None,
        };
        let tier = protocol.as_deref().and_then(|protocol| {
            orch_model_contract::parameters::requested_tier_for_caps(protocol, &config.params)
        });
        if !crate::caps::own_room(
            &mut tx,
            &crate::caps::OwnCapAsk {
                place: identity.place(),
                provider: config.provider_id,
                model: &config.model_name,
                tier: tier.as_deref(),
                purpose: "summary",
                person: request.requested_by,
            },
            self.inner.clock.now(),
        )
        .await?
        {
            return Ok(Some(Reason::CapNoRoom));
        }
        Ok(None)
    }

    /// `told` receives the reason when the source has nothing in it, so that no model is asked.
    async fn prepare_resource_work(
        &self,
        request: &rows::Request,
        token: Id,
        told: &mut Option<Reason>,
    ) -> Result<Work, EngineError> {
        if let Some(prepared) = &request.prepared {
            return serde_json::from_value(prepared.clone()).map_err(EngineError::from);
        }
        let target: SummaryTarget = serde_json::from_value(
            request
                .accepted_source
                .get("target")
                .cloned()
                .ok_or_else(refused)?,
        )?;
        let range: Option<SummaryChatRange> = serde_json::from_value(
            request
                .accepted_source
                .get("range")
                .cloned()
                .unwrap_or(Json::Null),
        )?;
        let body: orch_api_contract::summaries::SummaryRefreshBody =
            serde_json::from_value(request.arguments.clone())?;
        let accepted_model = request
            .accepted_source
            .get("model")
            .cloned()
            .map(serde_json::from_value::<SummaryModelChoice>)
            .transpose()?;
        let payer = request.paying_user.ok_or_else(refused)?;
        let mut tx = self.db().begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let http = request
            .accepted_source
            .get("http")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value::<operation_scope::http_origin::HttpOrigin>)
            .transpose()?;
        let caller = if let Some(http) = &http {
            // Material is read before the frame pins the original raw identity for its write.
            http.origin()
                .preview_current(&mut tx, self.inner.clock.as_ref())
                .await?
                .ok_or_else(refused)?
        } else if let Some(origin) = &request.accepted_operation {
            let origin: CapturedOperation = serde_json::from_value(origin.clone())?;
            if origin.user() != payer
                || origin.operation() != summary_sources::RESOURCE_REFRESH.operation
            {
                return Err(refused());
            }
            origin
                .preview_current(&mut tx, self.inner.clock.as_ref())
                .await?
                .ok_or_else(refused)?
        } else {
            Caller::session(payer).with_admin_mode(false)
        };
        let selected = select_with_snapshot(
            &mut tx,
            &caller,
            target,
            range,
            body.model.as_ref().or(accepted_model.as_ref()),
            self.inner.clock.time(),
            request.accepted_source.get("snapshot"),
        )
        .await?;
        if request.accepted_source.get("snapshot") != Some(&selected.material.snapshot)
            || request.accepted_source.get("policy")
                != Some(&serde_json::to_value(selected.policy.fingerprint)?)
        {
            return Err(refused());
        }
        // Nothing unread, nothing written, no activity: there is nothing to say, and a model asked to
        // summarize no material answers with a sentence about that, which would be kept as the summary.
        if selected.material.activity.is_empty() {
            *told = Some(Reason::NothingNew);
            return Err(refused());
        }
        let plan = operation_scope::sources::bind_required(
            SummaryRefresh::ROUTE,
            &summary_sources::RESOURCE_REFRESH,
            &selected.facts(),
        )
        .map_err(|_| refused())?;
        let principal = Principal::load(
            &mut tx,
            plan.admit(&caller, false).map_err(|_| refused())?,
            self.inner.clock.time(),
        )
        .await?;
        require_model(
            &mut tx,
            &principal,
            selected.material.identity.place(),
            &selected.config,
        )
        .await?;
        let work = Work::capture(request, &principal, &selected)?;
        if !rows::prepare(
            &mut tx,
            request.id,
            token,
            self.inner.clock.now(),
            &serde_json::to_value(&work)?,
        )
        .await?
        {
            return Err(refused());
        }
        // Reprepare after all native source/actor locks; no model request is made in this transaction.
        work.frame(&mut tx, self.inner.clock.as_ref())
            .await?
            .into_parts()
            .first_effect
            .check(self.inner.clock.as_ref())
            .map_err(command::frame_error)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(work)
    }

    /// `told` receives what the work itself saw of the model: that it did not answer, or that its
    /// answer could not be used. Whatever else no longer holds is looked at again by the caller.
    async fn execute_resource_work(
        &self,
        work: &Work,
        told: &mut Option<Reason>,
    ) -> Result<(String, ProviderComplete), EngineError> {
        let mut tx = self.db().begin().await?;
        let frame = work.frame(&mut tx, self.inner.clock.as_ref()).await?;
        let selected = frame.value();
        let place = selected.material.identity.place();
        let node = orch_db::queries::hierarchy::node(&mut tx, place)
            .await?
            .ok_or_else(refused)?;
        let field = if matches!(work.target, SummaryTarget::ChatTitle { .. }) {
            SummaryField::Title
        } else {
            SummaryField::Summary
        };
        let request = SummaryRequest {
            field,
            node: NodeRef {
                id: place,
                kind: serde_json::from_value(json!(node.kind))?,
                name: if matches!(
                    work.target,
                    SummaryTarget::Thread { .. } | SummaryTarget::Topic { .. }
                ) {
                    Some(selected.material.title.clone())
                } else {
                    None
                },
                redacted: !matches!(
                    work.target,
                    SummaryTarget::Thread { .. } | SummaryTarget::Topic { .. }
                ),
            },
            // Prior paragraphs may have governors that no longer belong to this source. Only
            // freshly admitted native material enters this new request.
            previous: String::new(),
            activity: selected.material.activity.clone(),
            tag_choices: Vec::new(),
            limits: selected.policy.limits(),
            config: serde_json::to_value(&selected.config)?,
        };
        let source = CallSource {
            request_id: work.request,
            node_id: Some(place),
            for_user_id: work.issuer,
            ..CallSource::default()
        };
        let mut executors = Executors::default();
        let host = super::super::completion::Host {
            engine: self.clone(),
            writer: Some(work.payer),
            call: work.request,
            workflow: work.request,
            task: "resource_summary",
            resource: Some(work.clone()),
        };
        executors
            .register(
                orch_summarizer_model::capabilities::EXECUTOR,
                Arc::new(if field == SummaryField::Title {
                    orch_summarizer_model::capabilities::Executor::title(
                        Arc::new(host),
                        source.clone(),
                    )
                } else {
                    orch_summarizer_model::capabilities::Executor::new(
                        Arc::new(host),
                        source.clone(),
                    )
                }),
            )
            .map_err(EngineError::unreadable)?;
        let resolution = resolve_route(
            &mut tx,
            frame.principal(),
            place,
            &selected.config,
            executors.names(),
        )
        .await?;
        let route = resolution.route.ok_or_else(refused)?;
        let call = CapabilityCall {
            source,
            capability: "summary".into(),
            route,
            item: resolution.item,
            ledger_unit: resolution.ledger_unit,
            input: serde_json::to_value(request)?,
        };
        frame
            .into_parts()
            .first_effect
            .check(self.inner.clock.as_ref())
            .map_err(command::frame_error)?;
        tx.commit().await.map_err(DbError::from)?;
        let result = match executors.execute(call).await {
            Ok(result) => result,
            Err(error) => {
                *told = match error.code {
                    SourceErrorCode::Unavailable | SourceErrorCode::Timeout => {
                        Some(Reason::ModelFailed)
                    }
                    SourceErrorCode::TooLarge => Some(Reason::OutputCut),
                    SourceErrorCode::Malformed => Some(Reason::UnusableAnswer),
                    _ => None,
                };
                // An unusable paid answer uses the same call rules as every summary. A
                // precondition refusal with no answered command reserves no paid retry.
                let chosen = super::super::Choice {
                    kind: "model".into(),
                    config: serde_json::to_value(&work.config)?,
                    writer: Some(work.payer),
                };
                self.model_output_failed(
                    &chosen,
                    place,
                    work.request,
                    work.request,
                    "resource_summary",
                )
                .await?;
                return Err(EngineError::unreadable(error));
            }
        };
        let Some(text) = result
            .items
            .first()
            .and_then(|item| item.payload.get("text"))
            .and_then(Json::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
        else {
            *told = Some(Reason::UnusableAnswer);
            return Err(refused());
        };
        let mut conn = self.db().acquire().await?;
        let actual = crate::calls::read_root(&mut conn, place, Purpose::Summary, work.request)
            .await?
            .and_then(|row| {
                row.context["next_call"]
                    .as_str()
                    .and_then(|id| id.parse::<Id>().ok())
            })
            .unwrap_or(work.request);
        let body = self
            .inner
            .commands
            .retained_body(actual)
            .await?
            .ok_or_else(refused)?;
        let actual: ProviderComplete = serde_json::from_value(body)?;
        Ok((text, actual))
    }
}
