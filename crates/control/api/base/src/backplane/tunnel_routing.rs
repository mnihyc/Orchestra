//! Carrier placement and ephemeral attachment coordination. No tunnel payload enters here.

use super::*;

use orch_link_contract::gateways::GatewayInstance;

use orch_link_contract::gateways::RoutedTicket;

use orch_link_contract::gateways::TunnelAttach;

use orch_link_contract::gateways::TunnelGatewayStrategy;

use orch_link_contract::gateways::TunnelPlan;

use orch_link_contract::gateways::TunnelPrepareAnswer;

use orch_link_contract::gateways::TunnelPrepareAsk;

use orch_link_contract::gateways::TunnelRelease;

use orch_link_contract::gateways::TunnelRouteAnswer;

use orch_link_contract::tunnels::Ticket;

use orch_link_contract::tunnels::TicketClaims;

use orch_link_contract::tunnels::TicketSource;

use orch_link_contract::tunnels::TunnelKind;

use orch_types::new_id;
#[derive(Clone)]
pub struct GatewaySamples {
    measured_at: orch_types::Time,
    at: orch_types::Time,
    rtts: HashMap<String, u64>,
}

fn combined(
    gateway: &str,
    source: &HashMap<String, u64>,
    destination: &HashMap<String, u64>,
) -> Option<u128> {
    Some(u128::from(*source.get(gateway)?) + u128::from(*destination.get(gateway)?))
}

/// Stable ordering compares only two-sided samples; unknown RTT is never a synthetic zero.
fn placement_order(
    mut gateways: Vec<GatewayInstance>,
    source_primary: &str,
    destination_primary: &str,
    source: &HashMap<String, u64>,
    destination: &HashMap<String, u64>,
    strategy: TunnelGatewayStrategy,
    peer_can_attach: bool,
) -> Option<Vec<GatewayInstance>> {
    let default = gateways
        .iter()
        .position(|gateway| gateway.id == destination_primary)?;
    let measured_selection = peer_can_attach
        && strategy == TunnelGatewayStrategy::LowestCombinedRtt
        && gateways
            .iter()
            .any(|gateway| combined(&gateway.id, source, destination).is_some());
    let chosen = if measured_selection {
        gateways
            .iter()
            .enumerate()
            .filter_map(|(index, gateway)| {
                Some((
                    index,
                    (
                        combined(&gateway.id, source, destination)?,
                        gateway.id != destination_primary,
                        index,
                    ),
                ))
            })
            .min_by_key(|(_, key)| *key)
            .map_or(default, |(index, _)| index)
    } else {
        default
    };
    let preferred = gateways.remove(chosen);
    if !peer_can_attach {
        return Some(vec![preferred]);
    }
    gateways.sort_by_key(|gateway| {
        let score = combined(&gateway.id, source, destination);
        let source_first = !measured_selection && gateway.id == source_primary;
        (!source_first, score.is_none(), score.unwrap_or(u128::MAX))
    });
    gateways.insert(0, preferred);
    Some(gateways)
}

pub struct PendingAttachment {
    source: Id,
    source_session: Id,
    source_request: Id,
    destination: Id,
}

pub struct ToolGateway {
    source: Id,
    expires_at: i64,
    environment: Option<(Id, i64)>,
    gateway: String,
    plan: Option<TunnelPlan>,
    refusal: Option<(String, Vec<String>)>,
}

pub struct ToolGatewayView {
    pub plan: Option<TunnelPlan>,
    pub gateway: Option<String>,
    pub refusal: Option<(String, Vec<String>)>,
}

impl Link {
    fn sample_is_fresh(&self, at: orch_types::Time) -> bool {
        let age = self
            .inner
            .clock
            .time()
            .get()
            .signed_duration_since(at.get());
        age >= chrono::Duration::zero()
            && u64::try_from(age.num_milliseconds()).is_ok_and(|millis| {
                millis
                    <= self
                        .inner
                        .settings
                        .gateway_sample_max_age_secs
                        .saturating_mul(1_000)
            })
    }

    /// Only source-owned, successfully committed machine facts become bounded placement hints.
    pub fn committed_gateway_samples(&self, machine: Id, delivery: &Delivery) {
        if delivery.stream != Stream::Machine {
            return;
        }
        let Some(event) = delivery
            .events
            .iter()
            .rev()
            .find(|event| event.kind == EventKind::MachineFacts.as_str())
        else {
            return;
        };
        let Ok(Event::MachineFacts(facts)) = Event::decode(&event.kind, event.body.clone()) else {
            return;
        };
        let Some(entry) = self.loaded_entry(machine) else {
            return;
        };
        let Some(report) = facts.gateway else {
            lock(&entry.state).gateway_samples = None;
            return;
        };
        let Some(measured_at) = report.measured_at.filter(|at| self.sample_is_fresh(*at)) else {
            lock(&entry.state).gateway_samples = None;
            return;
        };
        let catalogue = self.inner.certificates.advertised_gateways();
        let mut rtts = HashMap::<String, u64>::new();
        for sample in report.measurements {
            if catalogue
                .iter()
                .any(|gateway| gateway.id == sample.gateway_id)
                && sample.round_trip_ms
                    <= u64::from(orch_link_contract::session::CONNECT_TIMEOUT_SECS) * 1_000
            {
                rtts.entry(sample.gateway_id)
                    .and_modify(|score| *score = (*score).min(sample.round_trip_ms))
                    .or_insert(sample.round_trip_ms);
            }
        }
        lock(&entry.state).gateway_samples = (!rtts.is_empty()).then(|| GatewaySamples {
            measured_at,
            at: self.inner.clock.time(),
            rtts,
        });
    }

    fn placement_samples(&self, machine: Id) -> Option<GatewaySamples> {
        let entry = self.loaded_entry(machine)?;
        if !entry.is_online() {
            return None;
        }
        let samples = lock(&entry.state).gateway_samples.clone();
        samples.filter(|samples| {
            self.sample_is_fresh(samples.at) && self.sample_is_fresh(samples.measured_at)
        })
    }

    async fn designated_plan(
        &self,
        claims: &TicketClaims,
    ) -> Result<Option<TunnelPlan>, orch_link_contract::commands::CommandError> {
        use orch_link_contract::commands::{CommandError, ErrorCode};
        if !matches!(claims.src, TicketSource::Machine(source) if source != claims.dst) {
            return Ok(None);
        }
        let place = if let Some(thread) = claims.thread_id {
            orch_settings::Place::Node(thread)
        } else if let Some(environment) = claims.tool_environment_id {
            orch_settings::Place::Environment(environment)
        } else {
            orch_settings::Place::System
        };
        let mut conn = self.inner.db.acquire().await.map_err(|_| {
            CommandError::new(
                ErrorCode::Internal,
                "the tunnel placement setting could not be read",
            )
        })?;
        let scoped = !matches!(place, orch_settings::Place::System);
        let value = orch_settings::get::<String>(
            &mut conn,
            place,
            orch_settings_contract::keys::GATEWAYS_TUNNEL_STRATEGY,
        )
        .await;
        // Transport-only tickets can have no live business place. They use the system's policy;
        // failures to read an existing place never silently discard its operator override.
        let value = match value {
            Err(orch_settings::Error::NotFound) if scoped => {
                orch_settings::get::<String>(
                    &mut conn,
                    orch_settings::Place::System,
                    orch_settings_contract::keys::GATEWAYS_TUNNEL_STRATEGY,
                )
                .await
            }
            value => value,
        }
        .map_err(|_| {
            CommandError::new(
                ErrorCode::Internal,
                "the tunnel placement setting could not be read",
            )
        })?;
        let strategy = match value.as_deref() {
            None | Some("lowest_combined_rtt") => TunnelGatewayStrategy::LowestCombinedRtt,
            Some("executor") => TunnelGatewayStrategy::Executor,
            Some(_) => {
                return Err(CommandError::new(
                    ErrorCode::Internal,
                    "the tunnel placement setting is not recognized",
                ));
            }
        };
        Ok(self.plan_for_claims(claims, strategy))
    }

    pub fn expire_tool_gateway_reports(&self) {
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

    /// A missing control connection means carrier liveness is unknown, not that payload necessarily stopped.
    pub fn gateway_connected(&self, gateway: &str) -> bool {
        lock(&self.inner.gateways).contains_key(gateway)
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

    pub fn send_primary(&self, machine: Id, frame: DownFrame) -> bool {
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

    fn plan_for_claims(
        &self,
        claims: &TicketClaims,
        strategy: TunnelGatewayStrategy,
    ) -> Option<TunnelPlan> {
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
        let source_samples = self.placement_samples(source);
        let destination_samples = self.placement_samples(claims.dst);
        let empty = HashMap::new();
        let source_rtts = source_samples
            .as_ref()
            .map_or(&empty, |samples| &samples.rtts);
        let destination_rtts = destination_samples
            .as_ref()
            .map_or(&empty, |samples| &samples.rtts);
        let can_attach = self.supports_tunnel_attachments(claims.dst);
        let has_comparable = gateways
            .iter()
            .any(|gateway| combined(&gateway.id, source_rtts, destination_rtts).is_some());
        let mut ordered = placement_order(
            gateways,
            &source_gateway,
            &destination_gateway,
            source_rtts,
            destination_rtts,
            strategy,
            can_attach,
        )?;
        let preferred_gateway = ordered.remove(0);
        let rule = if !can_attach {
            orch_api_contract::gateways::TunnelGatewayRule::ExecutorPeerCapability
        } else if strategy == TunnelGatewayStrategy::Executor {
            orch_api_contract::gateways::TunnelGatewayRule::ExecutorPinned
        } else if has_comparable {
            orch_api_contract::gateways::TunnelGatewayRule::LowestCombinedRtt
        } else {
            orch_api_contract::gateways::TunnelGatewayRule::ExecutorSamplesUnavailable
        };
        let decision = orch_api_contract::gateways::TunnelGatewayDecision {
            rule,
            source_measured_at: source_samples.as_ref().map(|samples| samples.measured_at),
            destination_measured_at: destination_samples
                .as_ref()
                .map(|samples| samples.measured_at),
            max_sample_age_secs: self.inner.settings.gateway_sample_max_age_secs,
            combined_round_trip_ms: combined(&preferred_gateway.id, source_rtts, destination_rtts)
                .and_then(|score| u64::try_from(score).ok()),
        };
        Some(TunnelPlan {
            decision,
            plan_id: new_id(),
            src: source,
            dst: claims.dst,
            preferred_gateway,
            common_fallbacks: ordered,
            catalogue_revision: revision,
        })
    }

    /// Resolve only metadata beside a newly signed ticket; authority and secret handling stay shared.
    pub async fn routed_ticket_value(
        &self,
        ticket: String,
    ) -> Result<Json, orch_link_contract::commands::CommandError> {
        let parsed = Ticket::parse(&ticket).ok();
        let mut route = None;
        if let Some(parsed) = &parsed {
            let claims = parsed.claims();
            if let TicketSource::Machine(source) = claims.src
                && self.supports_tunnel_attachments(source)
                && matches!(
                    claims.kind,
                    TunnelKind::Exec | TunnelKind::Mcp | TunnelKind::Llm | TunnelKind::Port
                )
            {
                route = self.designated_plan(claims).await?;
                if matches!(claims.kind, TunnelKind::Exec | TunnelKind::Mcp)
                    && let (Some(thread), Some(plan)) = (claims.thread_id, &route)
                {
                    let mut observed = lock(&self.inner.tool_gateways);
                    let same = observed.get_mut(&thread).filter(|existing| {
                        existing.source == source
                            && existing.environment
                                == claims
                                    .tool_environment_id
                                    .zip(claims.tool_environment_generation)
                            && existing
                                .plan
                                .as_ref()
                                .is_some_and(|previous| previous.dst == plan.dst)
                    });
                    if let Some(existing) = same {
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
            }
        }
        Ok(route
            .and_then(|route| {
                serde_json::to_value(RoutedTicket {
                    ticket: ticket.clone(),
                    route,
                })
                .ok()
            })
            .unwrap_or(Json::String(ticket)))
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

    pub fn tool_gateway_status(
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
    pub async fn tunnel_plan_for(&self, source: Id, ticket: &str) -> Option<TunnelPlan> {
        let claims = self.verified_tool_ticket(source, ticket)?;
        self.designated_plan(&claims).await.ok().flatten()
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

    pub async fn tunnel_control(&self, machine: Id, session: Id, frame: &UpFrame) -> bool {
        match frame {
            UpFrame::TunnelRouteAsk(request) => {
                let (plan, policy_error) = match self.verified_tool_ticket(machine, &request.ticket)
                {
                    Some(claims) => match self.designated_plan(&claims).await {
                        Ok(plan) => (plan, None),
                        Err(_) => (
                            None,
                            Some("the tunnel placement setting could not be read".to_owned()),
                        ),
                    },
                    None => (None, None),
                };
                self.send_primary(
                    machine,
                    DownFrame::TunnelRouteAnswer(TunnelRouteAnswer {
                        request_id: request.request_id,
                        detail: policy_error.or_else(|| {
                            plan.is_none()
                                .then(|| "the destination has no common primary placement".into())
                        }),
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

#[cfg(any(test, feature = "test-support"))]
pub mod placement_tests {

    use super::*;
    #[allow(clippy::unwrap_used, reason = "placement fixture assertion")]
    fn choose(
        source: &[(&str, u64)],
        destination: &[(&str, u64)],
        strategy: TunnelGatewayStrategy,
        peer: bool,
    ) -> Vec<String> {
        let gateways = ["eu", "asia", "us"]
            .map(|id| GatewayInstance {
                id: id.into(),
                label: id.into(),
                region: id.into(),
                endpoints: vec![],
            })
            .to_vec();
        let scores = |values: &[(&str, u64)]| {
            values
                .iter()
                .map(|(id, rtt)| ((*id).to_owned(), *rtt))
                .collect::<HashMap<_, _>>()
        };
        placement_order(
            gateways,
            "eu",
            "asia",
            &scores(source),
            &scores(destination),
            strategy,
            peer,
        )
        .unwrap()
        .into_iter()
        .map(|gateway| gateway.id)
        .collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn both_sides_choose_the_lowest_sum_and_ties_prefer_the_executor() {
        let source = [("eu", 5), ("asia", 190), ("us", 20)];
        let destination = [("eu", 180), ("asia", 5), ("us", 20)];
        assert_eq!(
            choose(
                &source,
                &destination,
                TunnelGatewayStrategy::default(),
                true
            ),
            ["us", "eu", "asia"]
        );
        assert_eq!(
            choose(
                &[("eu", 0), ("asia", 20)],
                &[("eu", 40), ("asia", 20)],
                TunnelGatewayStrategy::default(),
                true
            )[0],
            "asia"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_both_sides_choose_the_lowest_sum_and_ties_prefer_the_executor() {
        self::both_sides_choose_the_lowest_sum_and_ties_prefer_the_executor()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_missing_measurement_is_not_a_zero_and_an_operator_pin_is_honoured() {
        let source = [("eu", 5), ("us", 20)];
        let destination = [("asia", 5)];
        assert_eq!(
            choose(
                &source,
                &destination,
                TunnelGatewayStrategy::default(),
                true
            ),
            ["asia", "eu", "us"]
        );
        assert_eq!(
            choose(
                &source,
                &[("eu", 180), ("asia", 5), ("us", 20)],
                TunnelGatewayStrategy::Executor,
                true
            )[0],
            "asia"
        );
        assert_eq!(
            choose(
                &source,
                &[("us", 20)],
                TunnelGatewayStrategy::default(),
                false
            ),
            ["asia"]
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_missing_measurement_is_not_a_zero_and_an_operator_pin_is_honoured() {
        self::a_missing_measurement_is_not_a_zero_and_an_operator_pin_is_honoured()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn huge_scores_do_not_overflow_and_missing_candidates_never_win() {
        assert_eq!(
            choose(
                &[("eu", u64::MAX), ("asia", u64::MAX), ("us", u64::MAX)],
                &[("eu", u64::MAX), ("asia", u64::MAX), ("us", u64::MAX)],
                TunnelGatewayStrategy::default(),
                true
            )[0],
            "asia"
        );
        assert_eq!(
            choose(
                &[("us", 0)],
                &[("us", 0)],
                TunnelGatewayStrategy::default(),
                true
            )[0],
            "us"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_huge_scores_do_not_overflow_and_missing_candidates_never_win() {
        self::huge_scores_do_not_overflow_and_missing_candidates_never_win()
    }
}
