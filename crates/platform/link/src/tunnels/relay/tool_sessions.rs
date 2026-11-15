//! Pending live admission and scope changes share the relay's single short state-machine lock.
use super::*;
use orch_link_contract::tool_sessions::{ToolSessionQuery, ToolSessionReply, ToolSessionUpdate};

pub(super) enum ToolAction {
    Open {
        opener: End,
        host_template_jti: Option<Id>,
        retention: Option<orch_link_contract::gateways::TunnelRetention>,
    },
    Resume {
        from: Id,
        resume: Resume,
    },
}

impl ToolAction {
    fn opener(&self) -> End {
        match self {
            Self::Open { opener, .. } => *opener,
            Self::Resume { from, resume } => End::Machine {
                id: *from,
                channel: Some(resume.channel),
            },
        }
    }
}

pub(super) struct PendingTool {
    claims: TicketClaims,
    action: ToolAction,
    session: Option<Id>,
    since: Instant,
}

fn tool(kind: TunnelKind) -> bool {
    matches!(kind, TunnelKind::Exec | TunnelKind::Llm | TunnelKind::Mcp)
}

impl Relay {
    pub(super) fn pending_channel(&self, end: End) -> bool {
        self.tool_pending
            .values()
            .any(|pending| pending.action.opener() == end)
    }

    fn reject_tool(&self, action: &ToolAction, detail: &str) {
        let end = action.opener();
        let Some(channel) = Self::channel_of(end) else {
            return;
        };
        self.end_text(
            end,
            match action {
                ToolAction::Open { .. } => Frame::OpenErr(OpenErr {
                    channel,
                    code: OpenErrorCode::Refused,
                    detail: detail.into(),
                }),
                ToolAction::Resume { .. } => Frame::Close(Close {
                    channel,
                    code: CloseCode::Reset,
                    detail: detail.into(),
                }),
            },
        );
    }

    pub(super) fn ask_tool(&mut self, claims: TicketClaims, action: ToolAction) {
        if !self.tool_available || self.api.is_none() {
            return self.reject_tool(&action, "current tool session control is unavailable");
        }
        let opener = action.opener();
        let ticket = match &action {
            ToolAction::Open {
                retention: Some(retention),
                ..
            } => retention.ticket.clone(),
            ToolAction::Resume { resume, .. } => resume.ticket.clone(),
            _ => return self.reject_tool(&action, "the native ticket reference is missing"),
        };
        if self.pending_channel(opener) {
            return self.reject_tool(&action, "that channel already waits for tool admission");
        }
        if let End::Machine { id, .. } = opener {
            let waiting=self.tool_pending.values().filter(|pending|matches!(pending.action.opener(),End::Machine {id:owner,..} if owner==id)).count();
            if self.channels_of(id).saturating_add(waiting) >= self.limits.max_channels as usize {
                return self.reject_tool(&action, "too many channels wait for this machine");
            }
        }
        let session = match opener {
            End::Machine { id, .. } => self.sessions.get(&id).map(|session| session.session_id),
            End::Api { .. } => None,
        };
        if matches!(opener, End::Machine { .. }) && session.is_none() {
            return;
        }
        let n = orch_types::new_id();
        self.tool_pending.insert(
            n,
            PendingTool {
                claims: claims.clone(),
                action,
                session,
                since: Instant::now(),
            },
        );
        if let Some(api) = &self.api {
            api.text(BackplaneFrame::ToolSessionQuery(ToolSessionQuery {
                n,
                claims: Box::new(claims),
                ticket,
            }));
        }
    }

    pub(super) fn tool_answer(&mut self, reply: ToolSessionReply) {
        let Some(pending) = self.tool_pending.remove(&reply.n) else {
            return;
        };
        let opener = pending.action.opener();
        let current = match opener {
            End::Machine { id, .. } => {
                self.sessions.get(&id).map(|session| session.session_id) == pending.session
            }
            End::Api { .. } => true,
        };
        if !current {
            self.prune_tool_revisions();
            return;
        }
        let valid = self.tool_available
            && current
            && reply.allowed
            && pending.claims.tool_session.as_ref().is_some_and(|scope| {
                reply.revision >= scope.revision
                    && self
                        .tool_openings
                        .get(&scope.thread_id)
                        .is_none_or(|epoch| *epoch <= scope.opening_epoch)
            })
            && pending
                .claims
                .check(pending.claims.src, self.clock.time())
                .is_ok();
        if !valid {
            self.reject_tool(
                &pending.action,
                "the current session no longer admits this tool channel",
            );
            self.prune_tool_revisions();
            return;
        }
        if let Some(scope) = &pending.claims.tool_session {
            self.advance_tool_opening(scope.thread_id, scope.opening_epoch);
            self.tool_revisions
                .entry(scope.segment_id)
                .and_modify(|revision| *revision = (*revision).max(reply.revision))
                .or_insert(reply.revision);
        }
        match pending.action {
            ToolAction::Open {
                opener,
                host_template_jti,
                retention,
            } => {
                let occupied = match opener {
                    End::Machine {
                        id,
                        channel: Some(channel),
                    } => self.by_channel.contains_key(&(id, channel)),
                    End::Api { machine, channel } => {
                        self.by_api_channel.contains_key(&(machine, channel))
                    }
                    _ => true,
                };
                if !occupied {
                    self.start(opener, pending.claims, host_template_jti, retention);
                }
            }
            ToolAction::Resume { from, resume } => self.resume_current(from, resume, true),
        }
        self.prune_tool_revisions();
    }

    fn advance_tool_opening(&mut self, thread: Id, epoch: i64) {
        if self
            .tool_openings
            .get(&thread)
            .is_some_and(|current| *current >= epoch)
        {
            return;
        }
        let older = |claims: &TicketClaims| {
            claims
                .tool_session
                .as_ref()
                .is_some_and(|scope| scope.thread_id == thread && scope.opening_epoch < epoch)
        };
        let pending = self
            .tool_pending
            .iter()
            .filter(|(_, pending)| older(&pending.claims))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in pending {
            if let Some(pending) = self.tool_pending.remove(&id) {
                self.reject_tool(
                    &pending.action,
                    "a newer physical session replaced this opening",
                );
            }
        }
        let routes = self
            .routes
            .iter()
            .filter(|(_, route)| older(&route.claims))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for route in routes {
            self.close_tool_route(route, "a newer physical session replaced this opening");
        }
        self.tool_openings.insert(thread, epoch);
    }

    fn prune_tool_revisions(&mut self) {
        let active = self
            .routes
            .values()
            .filter_map(|route| {
                route
                    .claims
                    .tool_session
                    .as_ref()
                    .map(|scope| scope.segment_id)
            })
            .chain(self.tool_pending.values().filter_map(|pending| {
                pending
                    .claims
                    .tool_session
                    .as_ref()
                    .map(|scope| scope.segment_id)
            }))
            .collect::<std::collections::HashSet<_>>();
        self.tool_revisions
            .retain(|segment, _| active.contains(segment));
        let threads = self
            .routes
            .values()
            .filter_map(|route| {
                route
                    .claims
                    .tool_session
                    .as_ref()
                    .map(|scope| scope.thread_id)
            })
            .chain(self.tool_pending.values().filter_map(|pending| {
                pending
                    .claims
                    .tool_session
                    .as_ref()
                    .map(|scope| scope.thread_id)
            }))
            .collect::<std::collections::HashSet<_>>();
        self.tool_openings
            .retain(|thread, _| threads.contains(thread));
    }

    fn close_tool_route(&mut self, id: RouteId, detail: &str) {
        if let Some(route) = self.routes.get(&id) {
            for end in [route.opener, route.destination] {
                if let Some(channel) = Self::channel_of(end) {
                    self.end_text(
                        end,
                        Frame::Close(Close {
                            channel,
                            code: CloseCode::Reset,
                            detail: detail.into(),
                        }),
                    );
                }
            }
        }
        self.remove(id);
    }

    /// Loss of the live authority ends tool routes and pending questions before another reply can arrive.
    pub fn tool_control(&mut self, available: bool) {
        self.tool_available = available;
        if !available {
            for (_, pending) in self.tool_pending.drain().collect::<Vec<_>>() {
                self.reject_tool(&pending.action, "current tool session control was lost");
            }
            let routes = self
                .routes
                .iter()
                .filter(|(_, route)| tool(route.claims.kind))
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            for id in routes {
                self.close_tool_route(id, "current tool session control was lost");
            }
            self.tool_revisions.clear();
            self.tool_openings.clear();
        }
    }

    /// A stopping gateway forgets its routes; retained native sockets ask another live API before resuming.
    pub fn tool_gateway_stopping(&mut self) {
        self.tool_available = false;
        self.tool_pending.clear();
        let routes = self
            .routes
            .iter()
            .filter(|(_, route)| tool(route.claims.kind))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for route in routes {
            self.remove(route);
        }
        self.tool_revisions.clear();
        self.tool_openings.clear();
    }

    pub(super) fn tool_update(&mut self, update: ToolSessionUpdate) {
        if update.opening_epoch <= 0
            || self
                .tool_openings
                .get(&update.thread_id)
                .is_some_and(|epoch| *epoch > update.opening_epoch)
        {
            return;
        }
        self.advance_tool_opening(update.thread_id, update.opening_epoch);
        if self
            .tool_revisions
            .get(&update.segment_id)
            .is_some_and(|revision| *revision > update.revision)
        {
            return;
        }
        let current = |claims: &TicketClaims| {
            claims.tool_session.as_ref().is_none_or(|scope| {
                scope.segment_id != update.segment_id
                    || scope.revision > update.revision
                    || update.live
                        && update.opening_id == Some(scope.opening_id)
                        && scope.thread_id == update.thread_id
                        && update.members.iter().any(|member| {
                            member.member == scope.member && member.nonce == scope.nonce
                        })
            })
        };
        let pending = self
            .tool_pending
            .iter()
            .filter(|(_, pending)| !current(&pending.claims))
            .map(|(n, _)| *n)
            .collect::<Vec<_>>();
        for n in pending {
            if let Some(pending) = self.tool_pending.remove(&n) {
                self.reject_tool(&pending.action, "the tool session or member ended");
            }
        }
        let routes = self
            .routes
            .iter()
            .filter(|(_, route)| !current(&route.claims))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in routes {
            self.close_tool_route(id, "the tool session or member ended");
        }
        self.tool_revisions
            .entry(update.segment_id)
            .and_modify(|revision| *revision = (*revision).max(update.revision))
            .or_insert(update.revision);
        self.prune_tool_revisions();
    }

    pub(super) fn tool_tick(&mut self, now: Instant) {
        let expired = self
            .tool_pending
            .iter()
            .filter(|(_, pending)| now.saturating_duration_since(pending.since) >= self.tool_wait)
            .map(|(n, _)| *n)
            .collect::<Vec<_>>();
        for n in expired {
            if let Some(pending) = self.tool_pending.remove(&n) {
                self.reject_tool(
                    &pending.action,
                    "the API did not confirm the tool session in time",
                );
            }
        }
        self.prune_tool_revisions();
    }

    pub(super) fn cancel_tool_pending(&mut self, machine: Id, channel: Option<u32>) {
        self.tool_pending.retain(|_,pending|!matches!(pending.action.opener(),End::Machine {id,channel:own} if id==machine && channel.is_none_or(|number|own==Some(number))));
        self.prune_tool_revisions();
    }
}
