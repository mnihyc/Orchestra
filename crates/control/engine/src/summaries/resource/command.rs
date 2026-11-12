//! Exact native summary request evidence in the shared durable command authority channel.
use super::*;
use crate::{
    commands::Outgoing,
    operation_scope::continuing::{Error, Source as NativeSource},
};
use futures::future::BoxFuture;
use orch_api_contract::{operation_sources::Declaration, routes::Route};
use orch_db::queries::{
    hierarchy,
    link::{CommandRow, NewCommand},
    threads,
};
use orch_link_contract::commands::{CommandKind, SerialKey};
use orch_types::Clock;

pub(super) fn frame_error(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        other => EngineError::BeforeEffect(Box::new(other)),
    }
}

/// The same source factory serves dispatch, recovery and final paragraph publication.
pub struct Source;
impl NativeSource for Source {
    type Role = Role;
    type Input = Work;
    type Value = Selection;
    const ROUTE: Route = SummaryRefresh::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &summary_sources::RESOURCE_REFRESH;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        work: &'a Work,
        now: Time,
    ) -> BoxFuture<'a, Result<(Selection, Vec<operation_scope::sources::Facts<Role>>), Error>> {
        Box::pin(async move {
            let request = rows::request(conn, work.request)
                .await?
                .ok_or(Error::Refused)?;
            if !rows::belongs_to(conn, request.summary_id, work.target, work.issuer).await? {
                return Err(Error::Refused);
            }
            let http = request
                .accepted_source
                .get("http")
                .filter(|value| !value.is_null())
                .cloned()
                .map(serde_json::from_value::<operation_scope::http_origin::HttpOrigin>)
                .transpose()
                .map_err(|_| Error::Refused)?;
            if http != work.http {
                return Err(Error::Refused);
            }
            let original: Work =
                serde_json::from_value(request.prepared.clone().ok_or(Error::Refused)?)
                    .map_err(|_| Error::Refused)?;
            if !work.same_source(&original)
                || request.state != "running"
                || request.lease_until.is_none_or(|end| end <= now.get())
                || request.paying_user != Some(work.payer)
                || request.requested_by != work.issuer
            {
                return Err(Error::Refused);
            }
            if request.origin == "person"
                && request.accepted_operation.as_ref()
                    != Some(&serde_json::to_value(&work.origin).map_err(EngineError::from)?)
            {
                return Err(Error::Refused);
            }
            let selected = select_with_snapshot(
                conn,
                caller,
                work.target,
                work.range,
                Some(&work.model),
                now,
                Some(&work.source),
            )
            .await?;
            if selected.policy.fingerprint != work.policy
                || selected.material.snapshot != work.source
                || selected.material.source_refs != work.refs
            {
                return Err(Error::Refused);
            }
            let facts = selected.facts();
            Ok((selected, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        work: &'a Work,
        selected: &'a Selection,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if principal.caller().user() != work.payer {
                return Err(Error::Refused);
            }
            if work.issuer.is_none()
                && (!selected.policy.options.automatic
                    || selected
                        .policy
                        .producer
                        .as_ref()
                        .and_then(|producer| producer.writer)
                        != Some(work.payer))
            {
                return Err(Error::Refused);
            }
            if work.issuer.is_none()
                && orch_db::queries::hierarchy::with_ancestors(
                    conn,
                    &[selected.material.identity.place()],
                )
                .await?
                .iter()
                .any(|node| node.archived_at.is_some() || node.deleted_at.is_some())
            {
                return Err(Error::Refused);
            }
            for source in &work.refs {
                if !sources::readable(conn, principal, *source)
                    .await
                    .map_err(source_error)?
                {
                    return Err(Error::Refused);
                }
            }
            require_model(
                conn,
                principal,
                selected.material.identity.place(),
                &selected.config,
            )
            .await?;
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        work: &'a Work,
        _: &'a Selection,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            work.lock_source(conn).await?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Work,
        opened: &'a Selection,
        fresh: &'a Selection,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(opened.material.snapshot == fresh.material.snapshot
                && opened.material.identity.fact() == fresh.material.identity.fact()
                && opened.material.inputs == fresh.material.inputs
                && opened.config == fresh.config
                && opened.policy.fingerprint == fresh.policy.fingerprint)
        })
    }
}
impl Work {
    fn same_source(&self, original: &Self) -> bool {
        self.origin == original.origin
            && self.http == original.http
            && self.request == original.request
            && self.target == original.target
            && self.issuer == original.issuer
            && self.payer == original.payer
            && self.config == original.config
            && self.policy == original.policy
            && self.source == original.source
            && self.refs == original.refs
            && self.range == original.range
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        if let Some(http) = &self.http {
            http.lock_identity(conn).await?;
        }
        lock_sources(conn, self.target, &self.refs, self.model.provider_id).await?;
        // Match the journal's cache-before-request publication order.
        sqlx::query("SELECT s.id FROM resource_summaries s JOIN resource_summary_requests r ON r.summary_id=s.id WHERE r.id=$1 FOR UPDATE OF s").bind(self.request).fetch_optional(&mut *conn).await.map_err(DbError::from)?.ok_or_else(refused)?;
        sqlx::query("SELECT id FROM resource_summary_requests WHERE id=$1 FOR UPDATE")
            .bind(self.request)
            .fetch_optional(conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(refused)?;
        Ok(())
    }
}

/// The actual native source lock prefix, shared by admission and later continuation.
pub(super) async fn lock_sources(
    conn: &mut Conn,
    target: SummaryTarget,
    refs: &[SummaryTarget],
    provider: Id,
) -> Result<(), EngineError> {
    // Native document scope prefix precedes node/settings and the request/command rows.
    if let SummaryTarget::Document { id } = target {
        rows::lock_document_options(conn, id)
            .await?
            .ok_or_else(refused)?;
    }
    let mut nodes = Vec::new();
    let mut thread_ids = Vec::new();
    for target in refs {
        let identity = sources::identify(conn, *target)
            .await
            .map_err(source_error)?;
        nodes.extend(
            hierarchy::with_ancestors(conn, &[identity.place()])
                .await?
                .into_iter()
                .map(|row| row.id),
        );
        if let SummaryTarget::Thread { id } = target {
            thread_ids.push(*id);
        }
    }
    thread_ids.sort_unstable();
    thread_ids.dedup();
    if !thread_ids.is_empty() {
        threads::tools::lock_threads(conn, &thread_ids).await?;
    }
    nodes.sort_unstable();
    nodes.dedup();
    for node in nodes {
        hierarchy::lock(conn, node).await?;
    }
    if let SummaryTarget::Chat { id } | SummaryTarget::ChatTitle { id } = target {
        sqlx::query("SELECT id FROM chats WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(refused)?;
    }
    sqlx::query("SELECT id FROM providers WHERE id=$1 FOR SHARE")
        .bind(provider)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?
        .ok_or_else(refused)?;
    Ok(())
}

/// Exact command identity/body plus original native work; private and versioned by its owner.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    work: Option<Work>,
    automatic: Option<crate::summaries::completion::automatic::Source>,
    call: Id,
    machine: Id,
    body: Hash,
    endpoint: Vec<u8>,
}
impl std::fmt::Debug for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SummaryCommand([private])")
    }
}
impl Authority {
    pub(crate) fn belongs_to(&self, node: Id, call: Id, work: Option<&Work>) -> bool {
        self.call == call
            && match (&self.work, &self.automatic, work) {
                (Some(kept), None, Some(work)) => kept.same_source(work),
                (None, Some(source), None) => source.belongs_to(node, call),
                _ => false,
            }
    }
    pub(crate) async fn capture(
        conn: &mut Conn,
        work: Work,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.kind != CommandKind::ProviderComplete
            || command.key != SerialKey::Provider(work.model.provider_id)
        {
            return Err(refused());
        }
        let endpoint =
            orch_db::queries::semantic::endpoint_fingerprint(conn, work.model.provider_id)
                .await?
                .ok_or_else(refused)?;
        Ok(Self {
            work: Some(work),
            automatic: None,
            call: command.id,
            machine: command.machine,
            body: Hash::of(&serde_json::to_vec(&command.body)?),
            endpoint,
        })
    }
    pub(in crate::summaries) async fn capture_automatic(
        conn: &mut Conn,
        source: crate::summaries::completion::automatic::Source,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.kind != CommandKind::ProviderComplete
            || command.key != SerialKey::Provider(source.provider())
        {
            return Err(refused());
        }
        let endpoint = orch_db::queries::semantic::endpoint_fingerprint(conn, source.provider())
            .await?
            .ok_or_else(refused)?;
        Ok(Self {
            work: None,
            automatic: Some(source),
            call: command.id,
            machine: command.machine,
            body: Hash::of(&serde_json::to_vec(&command.body)?),
            endpoint,
        })
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match (&self.work, &self.automatic) {
            (Some(work), None) => work.lock_source(conn).await,
            (None, Some(source)) => source.lock_source(conn).await,
            _ => Err(refused()),
        }
    }
    async fn current(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let (id, machine, kind, serial, body) = (
            command.id,
            command.machine_id,
            command.kind,
            command.serial_key,
            command.body,
        );
        if id != self.call
            || machine != self.machine
            || kind != "provider.complete"
            || Hash::of(&serde_json::to_vec(body)?) != self.body
        {
            return Ok(None);
        }
        let provider = match (&self.work, &self.automatic) {
            (Some(work), None) => work.model.provider_id,
            (None, Some(source)) => source.provider(),
            _ => return Ok(None),
        };
        if serial != SerialKey::Provider(provider).wire().as_deref() {
            return Ok(None);
        }
        let Some(row) = resources::provider(conn, provider).await? else {
            return Ok(None);
        };
        if row.key_machine_id != Some(machine)
            || orch_db::queries::semantic::endpoint_fingerprint(conn, row.id)
                .await?
                .as_deref()
                != Some(self.endpoint.as_slice())
        {
            return Ok(None);
        }
        if let Some(source) = &self.automatic {
            return source.current(conn, command, clock).await;
        }
        let work = self.work.as_ref().ok_or_else(refused)?;
        type StoredCall = (Id, Option<Id>, Option<Id>, Option<Id>);
        let call:Option<StoredCall>=sqlx::query_as("SELECT provider_id,person_id,issued_by,entry_id FROM summary_calls WHERE id=$1 AND purpose='summary' AND cancel_reason IS NULL").bind(id).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        if call.as_ref().map(|row| (row.0, row.1, row.2))
            != Some((work.model.provider_id, Some(work.payer), work.issuer))
        {
            return Ok(None);
        }
        let request: ProviderComplete = serde_json::from_value(body.clone())?;
        if request.provider_id != work.model.provider_id
            || request.model_name != work.model.model
            || request.purpose != orch_link_contract::commands::CompletionPurpose::Summary
        {
            return Ok(None);
        }

        let model = resources::model(conn, request.provider_id, &request.model_name)
            .await?
            .filter(|row| row.enabled)
            .ok_or_else(refused)?;
        let parameters = orch_provider_contract::models::composed_model_parameters(
            &model.parameters,
            &model.groups,
            &model.option_layers,
            Some(&request.protocol),
        )
        .map_err(EngineError::unreadable)?;
        if !row.protocols.contains(&request.protocol)
            || request.model_parameters.as_ref() != Some(&parameters)
        {
            return Ok(None);
        }
        let frame = match work.frame(conn, clock).await {
            Ok(frame) => frame,
            Err(EngineError::BeforeEffect(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        if self.call != work.request
            || request.provider_id != work.config.provider_id
            || request.model_name != work.config.model_name
        {
            let Some(recovery) = crate::calls::read_root(
                conn,
                frame.value().material.identity.place(),
                orch_provider_contract::calls::Purpose::Summary,
                work.request,
            )
            .await?
            else {
                return Ok(None);
            };
            let policy =
                crate::calls::Policy::load(conn, frame.value().material.identity.place()).await?;
            if recovery.state != "running"
                || recovery.context["next_call"] != serde_json::json!(self.call)
                || !policy
                    .rules
                    .iter()
                    .any(|rule| serde_json::json!(rule) == recovery.rule)
            {
                return Ok(None);
            }
        }
        if super::model_entry(
            conn,
            frame.principal(),
            frame.value().material.identity.place(),
            &frame.value().config,
        )
        .await?
            != call.and_then(|row| row.3)
        {
            return Ok(None);
        }
        let tier = request.params.as_ref().and_then(|params| {
            orch_model_contract::parameters::requested_tier_for_caps(&request.protocol, params)
        });
        if !crate::caps::own_room(
            conn,
            &crate::caps::OwnCapAsk {
                place: frame.value().material.identity.place(),
                provider: request.provider_id,
                model: &request.model_name,
                tier: tier.as_deref(),
                purpose: "summary",
                person: work.issuer,
            },
            clock.now(),
        )
        .await?
        {
            return Ok(None);
        }
        let parts = frame.into_parts();
        if parts.first_effect.check(clock).is_err() {
            return Ok(None);
        }
        let caller = parts.principal.caller().clone();
        let end = caller.grant().and_then(orch_authz::TokenGrant::expires_at);
        Ok(Some(vec![(Some(caller), end)]))
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(conn, command, clock).await
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let native = NewCommand {
            id: command.id,
            machine_id: command.machine_id,
            kind: &command.kind,
            body: &command.body,
            serial_key: command.serial_key(),
            deadline_at: command.deadline_at,
        };
        self.current(conn, &native, clock).await
    }
}
