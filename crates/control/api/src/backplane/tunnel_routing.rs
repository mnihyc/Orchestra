//! Carrier placement and ephemeral attachment coordination. No tunnel payload enters here.

use super::*;
use orch_link_contract::gateways::{
    RoutedTicket, TunnelAttach, TunnelPlan, TunnelPrepareAnswer, TunnelPrepareAsk, TunnelRelease,
    TunnelRouteAnswer,
};
use orch_link_contract::tunnels::{Ticket, TicketClaims, TicketSource, TunnelKind};
use orch_types::new_id;

pub(super) struct PendingAttachment {
    source: Id,
    source_session: Id,
    source_request: Id,
    destination: Id,
}

pub(super) struct ToolGateway {
    source: Id,
    expires_at: i64,
    environment: Option<(Id, i64)>,
    gateway: String,
    plan: Option<TunnelPlan>,
    refusal: Option<(String, Vec<String>)>,
}

pub(crate) struct ToolGatewayView {
    pub(crate) plan: Option<TunnelPlan>,
    pub(crate) gateway: Option<String>,
    pub(crate) refusal: Option<(String, Vec<String>)>,
}

impl Link {
    pub(super) fn expire_tool_gateway_reports(&self) {
        let now = self.inner.clock.time().unix_millis().div_euclid(1_000);
        lock(&self.inner.tool_gateways).retain(|_, report| report.expires_at > now);
    }

    /// Existing primary registry only; gateways never receive another gateway's session table.
    #[must_use]
    pub fn gateway_of(&self, machine: Id) -> Option<String> {
        let entry = lock(&self.inner.machines)
            .get(&machine)
            .and_then(|cell| cell.get().cloned())?;
        let route = entry.route()?;
        lock(&self.inner.gateways)
            .iter()
            .find(|(_, gateway)| Arc::ptr_eq(gateway, &route.gateway))
            .map(|(id, _)| id.clone())
    }

    fn supports_tunnel_attachments(&self, machine: Id) -> bool {
        lock(&self.inner.machines)
            .get(&machine)
            .and_then(|cell| cell.get().cloned())
            .is_some_and(|entry| lock(&entry.state).supports_tunnel_attachments)
    }

    fn primary_session(&self, machine: Id) -> Option<Id> {
        lock(&self.inner.machines)
            .get(&machine)
            .and_then(|cell| cell.get().cloned())
            .and_then(|entry| entry.route())
            .map(|route| route.session_id)
    }

    pub(super) fn send_primary(&self, machine: Id, frame: DownFrame) -> bool {
        let entry = lock(&self.inner.machines)
            .get(&machine)
            .and_then(|cell| cell.get().cloned());
        entry.and_then(|entry| entry.route()).is_some_and(|route| {
            route.gateway.send(BackplaneFrame::Down(Down {
                machine_id: machine,
                frame,
            }))
        })
    }

    fn plan_for_claims(&self, claims: &TicketClaims) -> Option<TunnelPlan> {
        let TicketSource::Machine(source) = claims.src else {
            return None;
        };
        if source == claims.dst {
            return None;
        }
        let destination_gateway = self.gateway_of(claims.dst)?;
        let source_gateway = self.gateway_of(source)?;
        let mut gateways = self.inner.certificates.advertised_gateways();
        let revision = crate::certificates::gateway_catalogue_revision(&gateways);
        for gateway in &mut gateways {
            gateway.endpoints = gateway
                .endpoints
                .iter()
                .filter_map(orch_link_contract::gateways::public_endpoint)
                .collect();
        }
        let preferred = gateways
            .iter()
            .position(|gateway| gateway.id == destination_gateway)?;
        let preferred_gateway = gateways.remove(preferred);
        if let Some(index) = gateways
            .iter()
            .position(|gateway| gateway.id == source_gateway)
        {
            let near = gateways.remove(index);
            gateways.insert(0, near);
        }
        // Legacy peers can use their primary only; new sources may attach there.
        if !self.supports_tunnel_attachments(claims.dst) {
            gateways.retain(|gateway| gateway.id == destination_gateway);
        }
        Some(TunnelPlan {
            plan_id: new_id(),
            src: source,
            dst: claims.dst,
            preferred_gateway,
            common_fallbacks: gateways,
            catalogue_revision: revision,
        })
    }

    /// Resolve only metadata beside a newly signed ticket; authority and secret handling stay shared.
    pub(crate) fn routed_ticket_value(&self, ticket: String) -> Json {
        let parsed = Ticket::parse(&ticket).ok();
        let route = parsed.as_ref().and_then(|parsed| {
            let claims = parsed.claims();
            let TicketSource::Machine(source) = claims.src else {
                return None;
            };
            if !self.supports_tunnel_attachments(source)
                || !matches!(
                    claims.kind,
                    TunnelKind::Exec | TunnelKind::Mcp | TunnelKind::Llm | TunnelKind::Port
                )
            {
                return None;
            }
            let plan = self.plan_for_claims(claims)?;
            if matches!(claims.kind, TunnelKind::Exec | TunnelKind::Mcp)
                && let Some(thread) = claims.thread_id
            {
                let mut observed = lock(&self.inner.tool_gateways);
                if let Some(existing) = observed.get_mut(&thread).filter(|existing| {
                    existing.source == source
                        && existing.environment
                            == claims
                                .tool_environment_id
                                .zip(claims.tool_environment_generation)
                        && existing
                            .plan
                            .as_ref()
                            .is_some_and(|previous| previous.dst == plan.dst)
                }) {
                    existing.plan = Some(plan.clone());
                    existing.expires_at = claims.exp;
                } else {
                    observed.insert(
                        thread,
                        ToolGateway {
                            source,
                            expires_at: claims.exp,
                            environment: claims
                                .tool_environment_id
                                .zip(claims.tool_environment_generation),
                            gateway: String::new(),
                            plan: Some(plan.clone()),
                            refusal: None,
                        },
                    );
                }
            }
            Some(plan)
        });
        route
            .and_then(|route| {
                serde_json::to_value(RoutedTicket {
                    ticket: ticket.clone(),
                    route,
                })
                .ok()
            })
            .unwrap_or(Json::String(ticket))
    }

    /// A display fact is returned only to the current harness source, never an older provider binding.
    #[must_use]
    pub fn tool_gateway_for(&self, thread: Id, source: Id) -> Option<String> {
        lock(&self.inner.tool_gateways)
            .get(&thread)
            .filter(|observed| observed.source == source)
            .filter(|observed| !observed.gateway.is_empty())
            .map(|observed| observed.gateway.clone())
    }

    pub(crate) fn tool_gateway_status(
        &self,
        thread: Id,
        source: Id,
        destination: Id,
        environment: Id,
        generation: i64,
    ) -> Option<ToolGatewayView> {
        lock(&self.inner.tool_gateways)
            .get(&thread)
            .filter(|observed| {
                observed.source == source
                    && observed.environment == Some((environment, generation))
                    && observed
                        .plan
                        .as_ref()
                        .is_none_or(|plan| plan.dst == destination)
            })
            .map(|observed| ToolGatewayView {
                plan: observed.plan.clone(),
                gateway: (!observed.gateway.is_empty()).then(|| observed.gateway.clone()),
                refusal: observed.refusal.clone(),
            })
    }

    fn verified_tool_ticket(&self, source: Id, ticket: &str) -> Option<TicketClaims> {
        self.inner
            .tickets
            .verifier()
            .verify(
                ticket,
                TicketSource::Machine(source),
                self.inner.clock.time(),
            )
            .ok()
    }

    /// The same verified plan used by command expansion and transport-only continuations.
    #[must_use]
    pub fn tunnel_plan_for(&self, source: Id, ticket: &str) -> Option<TunnelPlan> {
        self.verified_tool_ticket(source, ticket)
            .and_then(|claims| self.plan_for_claims(&claims))
    }

    fn prepare_answer(
        &self,
        source: Id,
        session: Id,
        request: Id,
        ok: bool,
        detail: Option<String>,
    ) {
        if self.primary_session(source) == Some(session) {
            self.send_primary(
                source,
                DownFrame::TunnelPrepareAnswer(TunnelPrepareAnswer {
                    request_id: request,
                    ok,
                    detail,
                }),
            );
        }
    }

    fn prepare_tunnel(&self, source: Id, session: Id, request: &TunnelPrepareAsk) {
        let Some(claims) = self.verified_tool_ticket(source, &request.ticket) else {
            self.prepare_answer(
                source,
                session,
                request.request_id,
                false,
                Some("the original ticket is not valid".into()),
            );
            return;
        };
        if request.plan.src != source
            || request.plan.dst != claims.dst
            || !request
                .plan
                .gateways()
                .any(|gateway| gateway.id == request.gateway_id)
        {
            self.prepare_answer(
                source,
                session,
                request.request_id,
                false,
                Some("the placement is not the ticket's pair".into()),
            );
            return;
        }
        let Some(gateway) = self
            .inner
            .certificates
            .gateway_discovery()
            .gateways
            .into_iter()
            .find(|gateway| gateway.id == request.gateway_id)
        else {
            self.prepare_answer(
                source,
                session,
                request.request_id,
                false,
                Some("the planned gateway is no longer advertised".into()),
            );
            return;
        };
        if self.gateway_of(claims.dst).as_deref() == Some(gateway.id.as_str()) {
            self.prepare_answer(source, session, request.request_id, true, None);
            return;
        }
        if self.primary_session(claims.dst).is_none()
            || !self.supports_tunnel_attachments(claims.dst)
        {
            self.prepare_answer(
                source,
                session,
                request.request_id,
                false,
                Some(format!(
                    "{}: the peer's primary cannot prepare an auxiliary",
                    gateway.label
                )),
            );
            return;
        }
        let forwarded = new_id();
        {
            let mut pending = lock(&self.inner.tunnel_attachments);
            // Reuse the protocol's channel ceiling as the bounded global preparation queue.
            if pending.len() >= MAX_CHANNELS as usize {
                drop(pending);
                self.prepare_answer(
                    source,
                    session,
                    request.request_id,
                    false,
                    Some("the bounded attachment queue is full".into()),
                );
                return;
            }
            pending.insert(
                forwarded,
                PendingAttachment {
                    source,
                    source_session: session,
                    source_request: request.request_id,
                    destination: claims.dst,
                },
            );
        }
        let mut plan = request.plan.clone();
        for entry in
            std::iter::once(&mut plan.preferred_gateway).chain(plan.common_fallbacks.iter_mut())
        {
            if entry.id == gateway.id {
                *entry = gateway.clone();
            }
        }
        if !self.send_primary(
            claims.dst,
            DownFrame::TunnelAttach(Box::new(TunnelAttach {
                request_id: forwarded,
                plan,
                gateway_id: gateway.id,
                expires_at: claims.exp,
            })),
        ) {
            lock(&self.inner.tunnel_attachments).remove(&forwarded);
            self.prepare_answer(
                source,
                session,
                request.request_id,
                false,
                Some("the peer's primary is unavailable".into()),
            );
            return;
        }
        let link = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(u64::from(
                orch_link_contract::tunnels::OPEN_TIMEOUT_SECS,
            )))
            .await;
            if let Some(pending) = lock(&link.inner.tunnel_attachments).remove(&forwarded) {
                link.prepare_answer(
                    pending.source,
                    pending.source_session,
                    pending.source_request,
                    false,
                    Some("the peer did not confirm its attachment".into()),
                );
            }
        });
    }

    pub(super) fn tunnel_control(&self, machine: Id, session: Id, frame: &UpFrame) -> bool {
        match frame {
            UpFrame::TunnelRouteAsk(request) => {
                let plan = self
                    .verified_tool_ticket(machine, &request.ticket)
                    .and_then(|claims| self.plan_for_claims(&claims));
                self.send_primary(
                    machine,
                    DownFrame::TunnelRouteAnswer(TunnelRouteAnswer {
                        request_id: request.request_id,
                        detail: plan
                            .is_none()
                            .then(|| "the destination has no common primary placement".into()),
                        plan,
                    }),
                );
            }
            UpFrame::TunnelPrepareAsk(request) => self.prepare_tunnel(machine, session, request),
            UpFrame::TunnelAttachResult(answer) => {
                let owned = lock(&self.inner.tunnel_attachments)
                    .get(&answer.request_id)
                    .is_some_and(|pending| pending.destination == machine);
                if owned
                    && let Some(pending) =
                        lock(&self.inner.tunnel_attachments).remove(&answer.request_id)
                {
                    self.prepare_answer(
                        pending.source,
                        pending.source_session,
                        pending.source_request,
                        answer.ok,
                        answer.detail.clone(),
                    );
                }
            }
            UpFrame::TunnelRelease(release) => {
                if release.src == machine {
                    self.send_primary(
                        release.dst,
                        DownFrame::TunnelRelease(TunnelRelease {
                            plan_id: release.plan_id,
                            src: machine,
                            dst: release.dst,
                        }),
                    );
                }
            }
            UpFrame::TunnelSelected(selected) => {
                if let Some(claims) = self.verified_tool_ticket(machine, &selected.ticket)
                    && matches!(claims.kind, TunnelKind::Exec | TunnelKind::Mcp)
                    && self
                        .inner
                        .certificates
                        .advertised_gateways()
                        .iter()
                        .any(|gateway| gateway.id == selected.gateway_id)
                    && let Some(thread) = claims.thread_id
                {
                    lock(&self.inner.tool_gateways).insert(
                        thread,
                        ToolGateway {
                            source: machine,
                            expires_at: claims.exp,
                            environment: claims
                                .tool_environment_id
                                .zip(claims.tool_environment_generation),
                            gateway: selected.gateway_id.clone(),
                            plan: selected.plan.clone().filter(|plan| {
                                plan.src == machine
                                    && plan.dst == claims.dst
                                    && plan.plan_id == selected.plan_id
                            }),
                            refusal: None,
                        },
                    );
                }
            }
            UpFrame::TunnelFailed(failed) => {
                if let Some(claims) = self.verified_tool_ticket(machine, &failed.ticket)
                    && matches!(claims.kind, TunnelKind::Exec | TunnelKind::Mcp)
                    && failed.plan.src == machine
                    && failed.plan.dst == claims.dst
                    && let Some(thread) = claims.thread_id
                {
                    let known = self.inner.certificates.advertised_gateways();
                    let attempted: Vec<String> = failed
                        .attempted_gateways
                        .iter()
                        .filter(|id| known.iter().any(|gateway| &gateway.id == *id))
                        .cloned()
                        .collect();
                    let detail = attempted
                        .iter()
                        .filter_map(|id| {
                            let gateway = known.iter().find(|gateway| &gateway.id == id)?;
                            let side = failed
                                .failures
                                .iter()
                                .find(|failure| &failure.gateway_id == id)
                                .map(|failure| failure.side);
                            let why = match side {
                                Some(orch_link_contract::gateways::GatewayAttachSide::Source) => {
                                    "source could not attach"
                                }
                                Some(orch_link_contract::gateways::GatewayAttachSide::Peer) => {
                                    "peer could not attach"
                                }
                                _ => "one or both connectors could not attach",
                            };
                            Some(format!("{}: {why}", gateway.label))
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    lock(&self.inner.tool_gateways).insert(
                        thread,
                        ToolGateway {
                            source: machine,
                            expires_at: claims.exp,
                            environment: claims
                                .tool_environment_id
                                .zip(claims.tool_environment_generation),
                            gateway: String::new(),
                            plan: Some(failed.plan.clone()),
                            refusal: Some((
                                format!("No common gateway could attach both ends. {detail}"),
                                attempted,
                            )),
                        },
                    );
                }
            }
            _ => return false,
        }
        true
    }
}
