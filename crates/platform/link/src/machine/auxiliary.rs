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

pub(super) struct PoolConfig {
    pub(super) identity: ClientIdentity,
    pub(super) dialer: Arc<dyn Dialer>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) options: TunnelOptions,
    pub(super) gauge: Option<Arc<Gauge>>,
    pub(super) budget: Arc<tokio::sync::Semaphore>,
    pub(super) incoming: mpsc::UnboundedSender<crate::tunnels::channel::IncomingTunnel>,
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
    recovering: Mutex<std::collections::HashSet<Id>>,
    attach_slots: Arc<tokio::sync::Semaphore>,
    stop: CancellationToken,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TunnelPool").finish_non_exhaustive()
    }
}

impl Pool {
    fn hubs(&self) -> Vec<(Option<String>, TunnelHub)> {
        let mut hubs = Vec::new();
        if let Some(parent) = self.parent.upgrade() {
            let gateway = parent
                .gateway_id
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            hubs.push((gateway, parent.tunnels.clone()));
        }
        hubs.extend(
            self.carriers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .values()
                .map(|carrier| (Some(carrier.gateway.id.clone()), carrier.hub.clone())),
        );
        hubs
    }

    fn reported_retention(
        &self,
        retained: &crate::tunnels::channel::RetainedChannel,
        gateway: &str,
    ) {
        if !retained.opener {
            return;
        }
        let Some(plan) = retained.retention.route.as_ref() else {
            return;
        };
        self.selected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(plan.plan_id, gateway.into());
        let reported_plan = self
            .leases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&plan.plan_id)
            .map_or_else(|| plan.clone(), |(original, _)| original.clone());
        self.send(Frame::TunnelSelected(TunnelSelected {
            ticket: retained.retention.ticket.clone(),
            plan: Some(reported_plan),
            plan_id: plan.plan_id,
            gateway_id: gateway.into(),
        }));
    }

    pub(super) fn primary_ready(&self) {
        for (gateway, hub) in self.hubs() {
            if let Some(gateway) = gateway {
                for retained in hub
                    .retained_channels()
                    .into_iter()
                    .filter(|retained| retained.suspended.is_none())
                {
                    self.reported_retention(&retained, &gateway);
                }
            }
        }
    }

    /// A trusted relay asks for an existing socket; never offer a new native tool on this path.
    pub(super) fn adopt_resume(
        &self,
        target: &TunnelHub,
        gateway: &str,
        resume: &orch_link_contract::tunnels::Resume,
    ) {
        if !target.retained_supported() {
            return;
        }
        let port_control = self
            .parent
            .upgrade()
            .is_some_and(|parent| *parent.tunnels.port_control().borrow());
        for (_, held) in self.hubs() {
            let retained = held.retained_channel(resume.jti_channel);
            if retained.is_some_and(|retained| !retained.port || port_control)
                && held.transfer_retained(target, resume.jti_channel, gateway, Some(resume))
            {
                return;
            }
        }
    }

    pub(super) fn session_lost(self: &Arc<Self>, hub: &TunnelHub, failed: &str) {
        for retained in hub
            .retained_channels()
            .into_iter()
            .filter(|retained| retained.suspended.is_some())
        {
            if !self
                .recovering
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(retained.cid)
            {
                continue;
            }
            let pool = self.clone();
            let failed = failed.to_owned();
            tokio::spawn(async move {
                let cid = retained.cid;
                let remaining = retained.grace.saturating_sub(
                    retained
                        .suspended
                        .map_or(Duration::ZERO, |since| since.elapsed()),
                );
                tokio::select! {()=pool.stop.cancelled()=>{},_=tokio::time::timeout(remaining,pool.recover_retained(retained,failed))=>{}}
                pool.recovering
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&cid);
            });
        }
    }

    async fn recover_retained(
        self: &Arc<Self>,
        retained: crate::tunnels::channel::RetainedChannel,
        failed: String,
    ) {
        let Some(plan) = retained.retention.route.as_ref() else {
            return;
        };
        let _recovery_lease = self.lease(vec![plan.clone()]);
        let Ok(ticket) = Ticket::parse(&retained.retention.ticket) else {
            return;
        };
        let Some(since) = retained.suspended else {
            return;
        };
        let mut candidates = plan.gateways().collect::<Vec<_>>();
        candidates.sort_by_key(|gateway| gateway.id == failed);
        let mut retry = Backoff::default();
        loop {
            if !self
                .hubs()
                .iter()
                .any(|(_, hub)| hub.retained_channel(retained.cid).is_some())
            {
                return;
            }
            for gateway in &candidates {
                if since.elapsed() >= retained.grace {
                    return;
                }
                if retained.port
                    && !self
                        .parent
                        .upgrade()
                        .is_some_and(|parent| *parent.tunnels.port_control().borrow())
                {
                    return;
                }
                for (current, hub) in self.hubs() {
                    if let Some(active) = hub
                        .retained_channel(retained.cid)
                        .filter(|item| item.suspended.is_none())
                    {
                        if let Some(current) = current {
                            if current != failed {
                                self.unreference(plan.plan_id, &failed);
                            }
                            self.reported_retention(&active, &current);
                        }
                        return;
                    }
                }
                let Ok(target) = self.ensure(plan, &gateway.id, ticket.claims().exp).await else {
                    continue;
                };
                if !target.retained_supported() {
                    continue;
                }
                if retained.port {
                    let mut control = target.port_control();
                    let ready = async {
                        loop {
                            if *control.borrow_and_update() {
                                return true;
                            }
                            if control.changed().await.is_err() {
                                return false;
                            }
                        }
                    };
                    if !tokio::time::timeout(
                        Duration::from_secs(u64::from(
                            orch_link_contract::session::CONNECT_TIMEOUT_SECS,
                        )),
                        ready,
                    )
                    .await
                    .unwrap_or(false)
                    {
                        continue;
                    }
                }
                if retained.opener {
                    // A surviving primary can prepare a peer whose old transport alone was cut.
                    // A dead selected primary is not required: both ends also recover from their retained plan.
                    let _ = self
                        .prepare(&retained.retention.ticket, plan, &gateway.id)
                        .await;
                }
                let port_current = !retained.port
                    || self
                        .parent
                        .upgrade()
                        .is_some_and(|parent| *parent.tunnels.port_control().borrow());
                if !port_current {
                    return;
                }
                let moved = self.hubs().into_iter().any(|(_, held)| {
                    held.transfer_retained(&target, retained.cid, &gateway.id, None)
                });
                if !moved {
                    continue;
                }
                target.resume_retained(retained.cid);
                let remaining = retained.grace.saturating_sub(since.elapsed());
                let attempt = remaining.min(Duration::from_secs(u64::from(
                    orch_link_contract::tunnels::OPEN_TIMEOUT_SECS,
                )));
                if tokio::time::timeout(attempt, target.wait_resumed(retained.cid))
                    .await
                    .unwrap_or(false)
                {
                    self.unreference(plan.plan_id, &failed);
                    self.reported_retention(&retained, &gateway.id);
                    return;
                }
            }
            let remaining = retained.grace.saturating_sub(since.elapsed());
            if remaining.is_zero() {
                return;
            }
            tokio::time::sleep(retry.failed(None).min(remaining)).await;
        }
    }

    pub(super) fn sessions(&self) -> usize {
        self.carriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|carrier| matches!(*carrier.standing.borrow(), Standing::Ready))
            .count()
    }
    pub(super) fn new(parent: Weak<Shared>, config: PoolConfig) -> Arc<Self> {
        let PoolConfig {
            identity,
            dialer,
            clock,
            options,
            gauge,
            budget,
            incoming,
        } = config;
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
            recovering: Mutex::new(std::collections::HashSet::new()),
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

    /// Serialize an idle auxiliary's retirement before the same gateway gains the primary purpose.
    pub(super) fn prepare_primary(&self, gateway: &str) -> bool {
        let Some(parent) = self.parent.upgrade() else {
            return false;
        };
        let carriers = self.carriers.lock().unwrap_or_else(PoisonError::into_inner);
        if carriers
            .get(gateway)
            .is_some_and(|carrier| !carrier.hub.pause_if_idle())
        {
            return false;
        }
        *parent
            .primary_attaching
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(gateway.to_owned());
        true
    }

    /// A new measurement or operator selection can change an idle handoff's target.
    pub(super) fn unpause_other_gateways(&self, primary: Option<&str>) {
        for (id, carrier) in self
            .carriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            if Some(id.as_str()) != primary {
                carrier.hub.unpause_admissions();
            }
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
            if parent
                .primary_attaching
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_deref()
                == Some(id)
            {
                return Err("the gateway is becoming the primary attachment".into());
            }
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

    fn keep(self: Arc<Self>, carrier: Arc<Carrier>) -> futures::future::BoxFuture<'static, ()> {
        Box::pin(async move {
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
                        .map(|(key, certificate)| {
                            key.duplicate().map(|key| (key, certificate.clone()))
                        })
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
                    carrier.hub.session_up_at(
                        session.sender.clone(),
                        &carrier.gateway.id,
                        session.capabilities.iter().any(|capability| {
                            capability == orch_link_contract::gateways::RETAINED_RESUME_CAPABILITY
                        }),
                    );
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
                                if let Frame::Resume(resume) = frame.as_ref() {
                                    self.adopt_resume(&carrier.hub, &carrier.gateway.id, resume);
                                }
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
                    if !carrier.stop.is_cancelled() {
                        self.session_lost(&carrier.hub, &carrier.gateway.id);
                    }
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
        })
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
            let opened = hub.open_routed(ticket, &plan, host_peer.clone()).await;
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
                    hub.open_routed(ticket, &plan, host_peer.clone()).await
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
