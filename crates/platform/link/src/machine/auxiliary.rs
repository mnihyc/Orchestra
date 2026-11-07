//! Tunnel-only carriers. Durable events, commands, presence and content stay on the primary.

use super::*;
use orch_link_contract::gateways::{
    GatewayInstance, SessionPurpose, TunnelAttach, TunnelAttachResult, TunnelPlan,
    TunnelPrepareAnswer, TunnelPrepareAsk, TunnelRelease, TunnelRouteAnswer, TunnelRouteAsk,
    TunnelSelected,
};
use orch_link_contract::tunnels::{OpenErrorCode, Ticket, TicketSource, TunnelKind};
use std::collections::HashMap;
use std::sync::Weak;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
enum Standing {
    Connecting,
    Ready,
    Failed(String),
    Closed,
}

struct Carrier {
    gateway: GatewayInstance,
    hub: TunnelHub,
    stop: CancellationToken,
    standing: watch::Sender<Standing>,
    sender: Mutex<Option<SessionSender>>,
    references: Mutex<HashMap<Id, (Id, Id, i64)>>,
    idle_since: Mutex<Option<Instant>>,
}

struct Pending<'a, T> {
    entries: &'a Mutex<HashMap<Id, oneshot::Sender<T>>>,
    id: Id,
}
impl<T> Drop for Pending<'_, T> {
    fn drop(&mut self) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

pub(super) struct Pool {
    parent: Weak<Shared>,
    identity: ClientIdentity,
    dialer: Arc<dyn Dialer>,
    clock: Arc<dyn Clock>,
    options: TunnelOptions,
    gauge: Option<Arc<Gauge>>,
    budget: Arc<tokio::sync::Semaphore>,
    incoming: mpsc::UnboundedSender<crate::tunnels::channel::IncomingTunnel>,
    carriers: Mutex<HashMap<String, Arc<Carrier>>>,
    leases: Mutex<HashMap<Id, (TunnelPlan, usize)>>,
    routes: Mutex<HashMap<Id, oneshot::Sender<TunnelRouteAnswer>>>,
    prepares: Mutex<HashMap<Id, oneshot::Sender<TunnelPrepareAnswer>>>,
    selected: Mutex<HashMap<Id, String>>,
    attach_slots: Arc<tokio::sync::Semaphore>,
    stop: CancellationToken,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TunnelPool").finish_non_exhaustive()
    }
}

impl Pool {
    pub(super) fn sessions(&self) -> usize {
        self.carriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|carrier| matches!(*carrier.standing.borrow(), Standing::Ready))
            .count()
    }
    pub(super) fn new(
        parent: Weak<Shared>,
        identity: ClientIdentity,
        dialer: Arc<dyn Dialer>,
        clock: Arc<dyn Clock>,
        options: TunnelOptions,
        gauge: Option<Arc<Gauge>>,
        budget: Arc<tokio::sync::Semaphore>,
        incoming: mpsc::UnboundedSender<crate::tunnels::channel::IncomingTunnel>,
    ) -> Arc<Self> {
        Arc::new(Self {
            parent,
            identity,
            dialer,
            clock,
            options,
            gauge,
            budget,
            incoming,
            carriers: Mutex::new(HashMap::new()),
            leases: Mutex::new(HashMap::new()),
            routes: Mutex::new(HashMap::new()),
            prepares: Mutex::new(HashMap::new()),
            selected: Mutex::new(HashMap::new()),
            attach_slots: Arc::new(tokio::sync::Semaphore::new(
                orch_link_contract::version::MAX_CHANNELS as usize,
            )),
            stop: CancellationToken::new(),
        })
    }

    pub(super) fn start_idle(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let Some(pool) = weak.upgrade() else {
                    return;
                };
                let Some(parent) = pool.parent.upgrade() else {
                    pool.stop.cancel();
                    return;
                };
                if pool.stop.is_cancelled() {
                    return;
                }
                let now = pool.clock.time().unix_millis().div_euclid(1_000);
                let idle = Duration::from_secs(
                    parent
                        .tunnel_idle_secs
                        .load(std::sync::atomic::Ordering::Relaxed),
                );
                let mut carriers = pool.carriers.lock().unwrap_or_else(PoisonError::into_inner);
                carriers.retain(|_, carrier| {
                    let mut refs = carrier
                        .references
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    refs.retain(|_, (_, _, expires)| *expires > now);
                    let mut since = carrier
                        .idle_since
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    if !refs.is_empty() || carrier.hub.has_live_channels() {
                        *since = None;
                        return true;
                    }
                    let started = *since.get_or_insert_with(Instant::now);
                    if started.elapsed() < idle {
                        return true;
                    }
                    carrier.stop.cancel();
                    carrier.hub.reset_all();
                    if let Some(sender) = carrier
                        .sender
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .as_ref()
                    {
                        sender.end();
                    }
                    false
                });
            }
        });
    }

    fn send(&self, frame: Frame) -> bool {
        let Some(parent) = self.parent.upgrade() else {
            return false;
        };
        let sender = parent
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        sender.is_some_and(|sender| sender.send(frame).is_ok())
    }

    pub(super) fn lease(
        self: &Arc<Self>,
        plans: Vec<TunnelPlan>,
    ) -> Arc<crate::gateways::TunnelLease> {
        let ids = plans.iter().map(|plan| plan.plan_id).collect();
        {
            let mut leases = self.leases.lock().unwrap_or_else(PoisonError::into_inner);
            for plan in &plans {
                let entry = leases
                    .entry(plan.plan_id)
                    .or_insert_with(|| (plan.clone(), 0));
                entry.1 += 1;
            }
        }
        let weak = Arc::downgrade(self);
        Arc::new(crate::gateways::TunnelLease::new(ids, move || {
            let Some(pool) = weak.upgrade() else {
                return;
            };
            for plan in plans {
                let final_owner = {
                    let mut leases = pool.leases.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(entry) = leases.get_mut(&plan.plan_id) {
                        entry.1 = entry.1.saturating_sub(1);
                        if entry.1 == 0 {
                            leases.remove(&plan.plan_id);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if final_owner {
                    pool.release(&TunnelRelease {
                        plan_id: plan.plan_id,
                        src: plan.src,
                        dst: plan.dst,
                    });
                    pool.send(Frame::TunnelRelease(TunnelRelease {
                        plan_id: plan.plan_id,
                        src: plan.src,
                        dst: plan.dst,
                    }));
                }
            }
        }))
    }

    pub(super) fn release(&self, release: &TunnelRelease) {
        self.selected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&release.plan_id);
        for carrier in self
            .carriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            let mut refs = carrier
                .references
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if refs
                .get(&release.plan_id)
                .is_some_and(|(src, dst, _)| *src == release.src && *dst == release.dst)
            {
                refs.remove(&release.plan_id);
            }
        }
    }

    fn unreference(&self, plan: Id, gateway: &str) {
        if let Some(carrier) = self
            .carriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(gateway)
        {
            carrier
                .references
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&plan);
        }
    }

    pub(super) async fn close_gateway(&self, gateway: &str) {
        let carrier = self
            .carriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(gateway);
        if let Some(carrier) = carrier {
            carrier.stop.cancel();
            carrier.hub.reset_all();
            if let Some(sender) = carrier
                .sender
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            {
                sender.end();
            }
            let mut standing = carrier.standing.subscribe();
            let _ = tokio::time::timeout(
                Duration::from_secs(u64::from(orch_link_contract::session::CONNECT_TIMEOUT_SECS)),
                async {
                    loop {
                        if matches!(*standing.borrow_and_update(), Standing::Closed) {
                            return;
                        }
                        if standing.changed().await.is_err() {
                            return;
                        }
                    }
                },
            )
            .await;
        }
    }

    pub(super) fn shutdown(&self) {
        self.stop.cancel();
        for carrier in self
            .carriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            carrier.stop.cancel();
            carrier.hub.reset_all();
            if let Some(sender) = carrier
                .sender
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            {
                sender.end();
            }
        }
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.prepares
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    async fn ensure(
        self: &Arc<Self>,
        plan: &TunnelPlan,
        id: &str,
        expires: i64,
    ) -> Result<TunnelHub, String> {
        if !plan.gateways().any(|gateway| gateway.id == id) {
            return Err("the gateway is not in this pair's frozen plan".into());
        }
        let parent = self.parent.upgrade().ok_or("the machine link ended")?;
        if parent
            .gateway_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_deref()
            == Some(id)
        {
            return Ok(parent.tunnels.clone());
        }
        if parent
            .primary_attaching
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_deref()
            == Some(id)
        {
            let mut status = parent.status.clone();
            let waiting = async {
                loop {
                    if parent
                        .gateway_id
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .as_deref()
                        == Some(id)
                    {
                        return Ok(parent.tunnels.clone());
                    }
                    if status.changed().await.is_err() {
                        return Err("the primary link ended".to_owned());
                    }
                }
            };
            return tokio::time::timeout(
                Duration::from_secs(u64::from(orch_link_contract::session::CONNECT_TIMEOUT_SECS)),
                waiting,
            )
            .await
            .map_err(|_| "the primary attachment did not become ready".to_owned())?;
        }
        let gateway = parent
            .gateway
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .catalogue
            .iter()
            .find(|gateway| gateway.id == id)
            .cloned()
            .ok_or_else(|| "the planned gateway is no longer advertised".to_owned())?;
        let carrier = {
            let mut carriers = self.carriers.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(existing) = carriers.get(id).filter(|entry| !entry.stop.is_cancelled()) {
                existing.clone()
            } else {
                let carrier = Arc::new(Carrier {
                    gateway,
                    hub: TunnelHub::with_shared_budget(
                        self.options,
                        self.gauge.clone(),
                        Arc::clone(&self.budget),
                    ),
                    stop: self.stop.child_token(),
                    standing: watch::channel(Standing::Connecting).0,
                    sender: Mutex::new(None),
                    references: Mutex::new(HashMap::new()),
                    idle_since: Mutex::new(None),
                });
                carriers.insert(id.to_owned(), carrier.clone());
                let pool = Arc::clone(self);
                let held = carrier.clone();
                tokio::spawn(async move {
                    pool.keep(held).await;
                });
                carrier
            }
        };
        carrier
            .references
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(plan.plan_id, (plan.src, plan.dst, expires));
        let mut standing = carrier.standing.subscribe();
        let waiting = async {
            loop {
                let observed = standing.borrow_and_update().clone();
                match observed {
                    Standing::Ready => return Ok(carrier.hub.clone()),
                    Standing::Failed(detail) => return Err(detail),
                    Standing::Closed => return Err("the tunnel-only attachment closed".into()),
                    Standing::Connecting => {}
                }
                if standing.changed().await.is_err() {
                    return Err("the tunnel-only attachment ended".into());
                }
            }
        };
        tokio::time::timeout(
            Duration::from_secs(u64::from(orch_link_contract::session::CONNECT_TIMEOUT_SECS)),
            waiting,
        )
        .await
        .map_err(|_| format!("{} did not become ready", carrier.gateway.label))?
    }

    async fn keep(self: Arc<Self>, carrier: Arc<Carrier>) {
        let mut incoming = carrier.hub.incoming();
        let tx = self.incoming.clone();
        let parent = self.parent.clone();
        tokio::spawn(async move {
            while let Some(tunnel) = incoming.next().await {
                if tunnel.kind == TunnelKind::Port
                    && !parent
                        .upgrade()
                        .is_some_and(|parent| *parent.tunnels.port_control().borrow())
                {
                    tunnel.refuse(
                        OpenErrorCode::Refused,
                        "primary port control is unavailable",
                    );
                    continue;
                }
                if tx.send(tunnel).is_err() {
                    return;
                }
            }
        });
        let mut backoff = Backoff::default();
        while !carrier.stop.is_cancelled() {
            carrier.standing.send_replace(Standing::Connecting);
            let mut established = None;
            for endpoint in &carrier.gateway.endpoints {
                if carrier.stop.is_cancelled() {
                    break;
                }
                let proof = match self
                    .identity
                    .proof
                    .as_ref()
                    .map(|(key, certificate)| key.duplicate().map(|key| (key, certificate.clone())))
                    .transpose()
                {
                    Ok(proof) => proof,
                    Err(_) => break,
                };
                let identity = ClientIdentity {
                    purpose: SessionPurpose::Tunnel,
                    gateway_id: Some(carrier.gateway.id.clone()),
                    endpoints: None,
                    hosted_endpoints: None,
                    machine_id: self.identity.machine_id,
                    connector_version: self.identity.connector_version.clone(),
                    platform: self.identity.platform,
                    roles: self.identity.roles.clone(),
                    proof,
                };
                let attempt = async {
                    let connection = self
                        .dialer
                        .dial_tunnel(endpoint)
                        .await
                        .map_err(|error| error.to_string())?;
                    connect(connection, &identity, self.clock.as_ref())
                        .await
                        .map_err(|error| error.to_string())
                };
                let result = tokio::select! { () = carrier.stop.cancelled() => break, result = attempt => result };
                if let Ok(session) = result {
                    established = Some(session);
                    break;
                }
            }
            if let Some(mut session) = established {
                backoff.succeeded();
                *carrier
                    .sender
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(session.sender.clone());
                carrier.hub.session_up(session.sender.clone());
                carrier.standing.send_replace(Standing::Ready);
                loop {
                    let event = tokio::select! { () = carrier.stop.cancelled() => { session.sender.end(); break; }, event = session.inbox.next() => event };
                    match event {
                        Some(SessionEvent::Frame(frame))
                            if matches!(
                                frame.as_ref(),
                                Frame::Open(_)
                                    | Frame::OpenOk(_)
                                    | Frame::OpenErr(_)
                                    | Frame::Resume(_)
                                    | Frame::ResumeOk(_)
                                    | Frame::Credit(_)
                                    | Frame::ChAck(_)
                                    | Frame::Eof(_)
                                    | Frame::Close(_)
                                    | Frame::PtyResize(_)
                                    | Frame::PtyExit(_)
                                    | Frame::PortControl(_)
                            ) =>
                        {
                            carrier.hub.on_frame(*frame)
                        }
                        Some(SessionEvent::Frame(_)) => {
                            session
                                .sender
                                .bye(orch_link_contract::version::ByeCode::ProtocolError);
                            break;
                        }
                        Some(SessionEvent::Data(data)) => carrier.hub.on_data(data),
                        Some(SessionEvent::Datagram(packet)) => carrier.hub.on_datagram(packet),
                        Some(SessionEvent::Ended(_)) | None => break,
                    }
                }
                carrier.hub.session_down();
                *carrier
                    .sender
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;
            }
            carrier.standing.send_replace(Standing::Failed(format!(
                "{} is unavailable for tunnel-only attachment",
                carrier.gateway.label
            )));
            let delay = backoff.failed(None);
            tokio::select! { () = carrier.stop.cancelled() => break, () = tokio::time::sleep(delay) => {} }
        }
        carrier.hub.reset_all();
        carrier.standing.send_replace(Standing::Closed);
    }

    async fn route(&self, ticket: &str) -> Result<Option<TunnelPlan>, OpenError> {
        let id = orch_types::new_id();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.routes.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.len() >= orch_link_contract::version::MAX_CHANNELS as usize {
                return Err(OpenError::TooMany);
            }
            pending.insert(id, tx);
        }
        let _pending = Pending {
            entries: &self.routes,
            id,
        };
        if !self.send(Frame::TunnelRouteAsk(TunnelRouteAsk {
            request_id: id,
            ticket: ticket.to_owned(),
        })) {
            return Err(OpenError::NoSession);
        }
        let result = tokio::time::timeout(
            Duration::from_secs(u64::from(orch_link_contract::tunnels::OPEN_TIMEOUT_SECS)),
            rx,
        )
        .await
        .map_err(|_| OpenError::TimedOut)?
        .map_err(|_| OpenError::Interrupted)?;
        Ok(result.plan)
    }

    async fn prepare(&self, ticket: &str, plan: &TunnelPlan, gateway: &str) -> Result<(), String> {
        let id = orch_types::new_id();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.prepares.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.len() >= orch_link_contract::version::MAX_CHANNELS as usize {
                return Err("the bounded attachment queue is full".into());
            }
            pending.insert(id, tx);
        }
        let _pending = Pending {
            entries: &self.prepares,
            id,
        };
        if !self.send(Frame::TunnelPrepareAsk(Box::new(TunnelPrepareAsk {
            request_id: id,
            ticket: ticket.to_owned(),
            plan: plan.clone(),
            gateway_id: gateway.to_owned(),
        }))) {
            return Err("primary control is unavailable".into());
        }
        let reply = tokio::time::timeout(
            Duration::from_secs(u64::from(orch_link_contract::tunnels::OPEN_TIMEOUT_SECS)),
            rx,
        )
        .await
        .map_err(|_| "the peer did not confirm attachment".to_owned())?
        .map_err(|_| "primary control ended".to_owned())?;
        if reply.ok {
            Ok(())
        } else {
            Err(reply
                .detail
                .unwrap_or_else(|| "the peer could not attach".into()))
        }
    }

    pub(super) async fn open(
        self: &Arc<Self>,
        ticket: &str,
        plan: Option<&TunnelPlan>,
        host_peer: Option<orch_link_contract::ports::HostPortPeer>,
    ) -> Result<Tunnel, OpenError> {
        let parsed = Ticket::parse(ticket).map_err(|_| OpenError::Refused {
            code: OpenErrorCode::BadTicket,
            detail: "the ticket could not be read".into(),
        })?;
        let claims = parsed.claims();
        let plan = match plan {
            Some(plan) => plan.clone(),
            None => self
                .route(ticket)
                .await?
                .ok_or_else(|| OpenError::Refused {
                    code: OpenErrorCode::PeerOffline,
                    detail: "the destination has no primary connection".into(),
                })?,
        };
        if claims.src != TicketSource::Machine(self.identity.machine_id)
            || plan.src != self.identity.machine_id
            || plan.dst != claims.dst
        {
            return Err(OpenError::Refused {
                code: OpenErrorCode::BadTicket,
                detail: "the routing plan is not the ticket's pair".into(),
            });
        }
        let _use_lease = self.lease(vec![plan.clone()]);
        let mut failures = Vec::new();
        let mut sides = Vec::new();
        for gateway in plan.gateways() {
            let hub = match self.ensure(&plan, &gateway.id, claims.exp).await {
                Ok(hub) => hub,
                Err(reason) => {
                    self.unreference(plan.plan_id, &gateway.id);
                    sides.push(orch_link_contract::gateways::GatewayAttachFailure {
                        gateway_id: gateway.id.clone(),
                        side: orch_link_contract::gateways::GatewayAttachSide::Source,
                    });
                    failures.push(format!(
                        "{}: source could not attach ({reason})",
                        gateway.label
                    ));
                    continue;
                }
            };
            let opened = match host_peer.clone() {
                Some(peer) => hub.open_host_port(ticket, peer).await,
                None => hub.open(ticket).await,
            };
            let opened = match opened {
                Err(OpenError::Refused {
                    code: OpenErrorCode::PeerOffline,
                    ..
                }) => {
                    if let Err(reason) = self.prepare(ticket, &plan, &gateway.id).await {
                        self.unreference(plan.plan_id, &gateway.id);
                        sides.push(orch_link_contract::gateways::GatewayAttachFailure {
                            gateway_id: gateway.id.clone(),
                            side: orch_link_contract::gateways::GatewayAttachSide::Peer,
                        });
                        failures.push(format!(
                            "{}: peer could not attach ({reason})",
                            gateway.label
                        ));
                        continue;
                    }
                    match host_peer.clone() {
                        Some(peer) => hub.open_host_port(ticket, peer).await,
                        None => hub.open(ticket).await,
                    }
                }
                other => other,
            };
            match opened {
                Ok(tunnel) => {
                    let changed = self
                        .selected
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(plan.plan_id, gateway.id.clone())
                        .as_deref()
                        != Some(gateway.id.as_str());
                    if changed && matches!(claims.kind, TunnelKind::Exec | TunnelKind::Mcp) {
                        self.send(Frame::TunnelSelected(TunnelSelected {
                            plan: Some(plan.clone()),
                            ticket: ticket.to_owned(),
                            plan_id: plan.plan_id,
                            gateway_id: gateway.id.clone(),
                        }));
                    }
                    return Ok(tunnel);
                }
                Err(
                    error @ (OpenError::Refused {
                        code:
                            OpenErrorCode::BadTicket
                            | OpenErrorCode::Expired
                            | OpenErrorCode::NoSuchService
                            | OpenErrorCode::Refused
                            | OpenErrorCode::TooManyChannels,
                        ..
                    }
                    | OpenError::TooMany),
                ) => return Err(error),
                Err(_) => {
                    self.unreference(plan.plan_id, &gateway.id);
                    sides.push(orch_link_contract::gateways::GatewayAttachFailure {
                        gateway_id: gateway.id.clone(),
                        side: orch_link_contract::gateways::GatewayAttachSide::Connection,
                    });
                    failures.push(format!("{}: peer is unreachable", gateway.label));
                }
            }
        }
        let detail = format!("tunnel.gateway_unreachable: {}", failures.join("; "));
        if matches!(claims.kind, TunnelKind::Exec | TunnelKind::Mcp) {
            self.send(Frame::TunnelFailed(Box::new(
                orch_link_contract::gateways::TunnelFailed {
                    failures: sides,
                    ticket: ticket.to_owned(),
                    plan: plan.clone(),
                    attempted_gateways: plan.gateways().map(|gateway| gateway.id.clone()).collect(),
                    detail: detail.clone(),
                },
            )));
        }
        Err(OpenError::Refused {
            code: OpenErrorCode::GatewayUnreachable,
            detail,
        })
    }

    pub(super) fn control(self: &Arc<Self>, frame: &Frame) -> bool {
        match frame {
            Frame::TunnelRouteAnswer(answer) => {
                if let Some(tx) = self
                    .routes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&answer.request_id)
                {
                    let _ = tx.send(answer.clone());
                }
            }
            Frame::TunnelPrepareAnswer(answer) => {
                if let Some(tx) = self
                    .prepares
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&answer.request_id)
                {
                    let _ = tx.send(answer.clone());
                }
            }
            Frame::TunnelRelease(release) => self.release(release),
            Frame::TunnelAttach(request) => {
                if request.plan.dst != self.identity.machine_id {
                    return true;
                }
                let permit = match Arc::clone(&self.attach_slots).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        self.send(Frame::TunnelAttachResult(TunnelAttachResult {
                            request_id: request.request_id,
                            ok: false,
                            detail: Some("the bounded attachment queue is full".into()),
                        }));
                        return true;
                    }
                };
                let pool = Arc::clone(self);
                let request: TunnelAttach = request.as_ref().clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let result = pool
                        .ensure(&request.plan, &request.gateway_id, request.expires_at)
                        .await;
                    if result.is_err() {
                        pool.unreference(request.plan.plan_id, &request.gateway_id);
                    }
                    pool.send(Frame::TunnelAttachResult(TunnelAttachResult {
                        request_id: request.request_id,
                        ok: result.is_ok(),
                        detail: result.err(),
                    }));
                });
            }
            _ => return false,
        }
        true
    }
}
