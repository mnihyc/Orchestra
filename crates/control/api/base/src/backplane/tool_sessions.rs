//! Tool channels ask current native session facts; notifications withdraw their old admissions.
use super::*;
use orch_link_contract::tool_sessions::{
    DEFAULT_ADMISSION_TIMEOUT_MS, ToolSessionControl, ToolSessionMember, ToolSessionQuery,
    ToolSessionReply, ToolSessionUpdate,
};
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_settings_contract::registry::keys;

const CHANNEL: &str = "orch_tool_session";

impl Link {
    /// This is also used by the native admission cases, independently of the gateway listener.
    pub async fn authorize_tool_session(&self, query: &ToolSessionQuery) -> ToolSessionReply {
        let denied = ToolSessionReply {
            n: query.n,
            allowed: false,
            revision: 0,
        };
        let verified = match self.inner.tickets.verifier().verify(
            &query.ticket,
            query.claims.src,
            self.inner.clock.time(),
        ) {
            Ok(claims) if claims == *query.claims => claims,
            _ => return denied,
        };
        let (TicketSource::Machine(source), Some(scope)) =
            (verified.src, verified.tool_session.as_ref())
        else {
            return denied;
        };
        let service = match verified.service.parse::<Service>() {
            Ok(service) if scope.accepts(&service) => service,
            _ => return denied,
        };
        let result = async {
            let mut tx = self.inner.db.begin().await?;
            // The same row is changed by close, replacement and every durable withdrawal. Holding it
            // until the answer is decided gives those changes one order without holding a relay lock.
            let facts:Option<Json> = sqlx::query_scalar(
                "SELECT s.delivered_resources->'tool_admission' FROM segments s
                   JOIN threads t ON t.node_id=s.thread_id AND t.current_segment_id=s.id
                   JOIN nodes n ON n.id=t.node_id
                   JOIN environments e ON e.id=s.environment_id AND t.environment_id=e.id
                   JOIN providers p ON p.id=s.provider_id
                   JOIN agents a ON a.id=s.agent_id
                   JOIN machines source ON source.id=s.machine_id
                   JOIN machines target ON target.id=$4
                  WHERE s.id=$1 AND s.thread_id=$2 AND s.machine_id=$3 AND s.ended_at IS NULL
                    AND e.deleted_at IS NULL AND e.lost_at IS NULL AND e.generation=s.env_generation
                    AND e.status IN ('running','offline')
                    AND p.deleted_at IS NULL AND p.status<>'disabled' AND a.deleted_at IS NULL AND NOT a.disabled
                    AND source.status='active' AND source.deleted_at IS NULL AND source.lost_at IS NULL
                    AND target.status='active' AND target.deleted_at IS NULL AND target.lost_at IS NULL
                    AND NOT EXISTS(SELECT 1 FROM nodes ancestor WHERE ancestor.id=ANY(n.path)
                                   AND (ancestor.deleted_at IS NOT NULL OR ancestor.archived_at IS NOT NULL))
                  FOR SHARE OF s",
            ).bind(scope.segment_id).bind(scope.thread_id).bind(source).bind(verified.dst)
                .fetch_optional(&mut *tx).await?.flatten();
            let Some(facts) = facts else { return Ok::<_,DbError>(None); };
            let revision=facts.get("revision").and_then(Json::as_i64).unwrap_or_default();
            let expected=facts.get("members").and_then(|members|members.get(&scope.member));
            let matches_id=|value:&Json,key:&str,wanted:Id| value.get(key).and_then(Json::as_str).and_then(|text|text.parse::<Id>().ok())==Some(wanted);
            if facts.get("live").and_then(Json::as_bool)!=Some(true) || revision<scope.revision
                || !matches_id(&facts,"thread_id",scope.thread_id) || !matches_id(&facts,"segment_id",scope.segment_id)
                || !matches_id(&facts,"opening_id",scope.opening_id) || !matches_id(&facts,"machine_id",source)
                || !expected.is_some_and(|member|matches_id(member,"nonce",scope.nonce)) {
                return Ok(None);
            }
            let request=expected.and_then(|member|member.pointer("/binding/ticket/$ticket"));
            let Some(request)=request else {return Ok(None);};
            let text=|key:&str|request.get(key).and_then(Json::as_str);
            if text("service")!=Some(verified.service.as_str()) || !matches_id(request,"dst",verified.dst)
                || !matches_id(request,"thread_id",scope.thread_id)
                || text("tool_set")!=verified.tool_set.as_deref()
                || text("executor_version")!=verified.tool_executor_version.as_deref() {
                return Ok(None);
            }
            match service {
                Service::Exec {..}|Service::Mcp {..} => {
                    let environment:Option<(Id,Id,i32)>=sqlx::query_as(
                        "SELECT e.id,e.machine_id,e.generation FROM environments e JOIN segments s ON s.environment_id=e.id WHERE s.id=$1 AND e.machine_id IS NOT NULL",
                    ).bind(scope.segment_id).fetch_optional(&mut *tx).await?;
                    let Some((environment,machine,generation))=environment else {return Ok(None);};
                    if verified.tool_environment_id!=Some(environment) || verified.tool_environment_generation!=Some(i64::from(generation))
                        || verified.dst!=machine || !matches_id(request,"environment_id",environment)
                        || request.get("environment_generation").and_then(Json::as_i64)!=Some(i64::from(generation)) {
                        return Ok(None);
                    }
                    if matches!(service,Service::Mcp {..}) && scope.member.starts_with("mcp:") {
                        let server=scope.member.strip_prefix("mcp:").and_then(|text|text.parse::<Id>().ok());
                        let current=orch_engine::mcp::resolve(&mut tx,environment,self.inner.clock.time()).await
                            .map_err(|_|DbError::Rule {constraint:"tool_membership_read".into()})?;
                        let service_name=match &service {Service::Mcp {server}=>Some(server.as_str()),_=>None};
                        if !current.iter().any(|member|Some(member.row.id)==server && Some(member.row.name.as_str())==service_name && member.reason.is_none()) {
                            return Ok(None);
                        }
                    }
                }
                Service::Llm {lease_id} => {
                    let live:bool=sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM leases l JOIN segments s ON s.thread_id=l.thread_id AND s.provider_id=l.provider_id JOIN providers p ON p.id=l.provider_id WHERE s.id=$1 AND l.id=$2 AND l.closed_at IS NULL AND p.key_machine_id=$3)",
                    ).bind(scope.segment_id).bind(lease_id).bind(verified.dst).fetch_one(&mut *tx).await?;
                    if !live {return Ok(None);}
                }
                _=>return Ok(None),
            }
            tx.commit().await?;
            Ok(Some(revision))
        }.await;
        match result {
            Ok(Some(revision)) => ToolSessionReply {
                n: query.n,
                allowed: true,
                revision,
            },
            Ok(None) => denied,
            Err(error) => {
                tracing::warn!(%error,"a tool channel could not read its current session");
                denied
            }
        }
    }

    fn publish_tool_control(&self, available: bool) {
        self.inner.tool_ready.store(available, Ordering::Release);
        let frame = BackplaneFrame::ToolSessionControl(ToolSessionControl {
            available,
            admission_timeout_ms: self.inner.tool_wait_ms.load(Ordering::Acquire),
        });
        for gateway in lock(&self.inner.gateways).values() {
            gateway.send(frame.clone());
        }
    }

    async fn read_tool_control(&self) -> Result<(), DbError> {
        let mut conn = self.inner.db.acquire().await?;
        let wait = orch_settings::get::<u64>(
            &mut conn,
            orch_settings::Place::System,
            keys::TUNNELS_CONTROL_TIMEOUT_MS,
        )
        .await
        .map_err(|_| DbError::Rule {
            constraint: "tool_admission_wait".into(),
        })?
        .unwrap_or(DEFAULT_ADMISSION_TIMEOUT_MS);
        if wait == 0 {
            return Err(DbError::Rule {
                constraint: "tool_admission_wait".into(),
            });
        }
        self.inner.tool_wait_ms.store(wait, Ordering::Release);
        Ok(())
    }

    async fn publish_tool_session(&self, segment: Id) -> Result<(), DbError> {
        let mut conn = self.inner.db.acquire().await?;
        let facts: Option<Json> = sqlx::query_scalar(
            "SELECT delivered_resources->'tool_admission' FROM segments WHERE id=$1",
        )
        .bind(segment)
        .fetch_optional(&mut *conn)
        .await?
        .flatten();
        if let Some(facts) = facts {
            let read_id = |key: &str| {
                facts
                    .get(key)
                    .and_then(Json::as_str)
                    .and_then(|text| text.parse::<Id>().ok())
            };
            if let (Some(thread), Some(opening), Some(revision)) = (
                read_id("thread_id"),
                read_id("opening_id"),
                facts.get("revision").and_then(Json::as_i64),
            ) {
                let members = facts
                    .get("members")
                    .and_then(Json::as_object)
                    .into_iter()
                    .flatten()
                    .filter_map(|(key, value)| {
                        Some(ToolSessionMember {
                            member: key.clone(),
                            nonce: value.get("nonce")?.as_str()?.parse().ok()?,
                        })
                    })
                    .collect();
                self.broadcast_tool_session(ToolSessionUpdate {
                    thread_id: thread,
                    segment_id: segment,
                    opening_id: Some(opening),
                    revision,
                    live: facts.get("live").and_then(Json::as_bool) == Some(true),
                    members,
                });
            }
        }
        Ok(())
    }

    fn broadcast_tool_session(&self, update: ToolSessionUpdate) {
        for gateway in lock(&self.inner.gateways).values() {
            gateway.send(BackplaneFrame::ToolSessionUpdate(update.clone()));
        }
    }

    pub(super) async fn prepare_tool_control(
        &self,
    ) -> Result<orch_db::sequences::Notices, DbError> {
        let notices = orch_db::sequences::listen(&self.inner.db, &[CHANNEL]).await?;
        self.read_tool_control().await?;
        self.publish_tool_control(true);
        Ok(notices)
    }

    pub(super) async fn tool_session_loop(self, mut initial: Option<orch_db::sequences::Notices>) {
        let mut backoff = orch_link::reconnect::Backoff::default();
        loop {
            if self.inner.shutdown.is_cancelled() {
                break;
            }
            // Reconnecting cannot retain an old positive route: missed announcements are unknowable.
            let result = async {
                let mut notices = match initial.take() {
                    Some(notices) => notices,
                    None => {
                        self.publish_tool_control(false);
                        orch_db::sequences::listen(&self.inner.db, &[CHANNEL]).await?
                    }
                };
                self.read_tool_control().await?;
                self.publish_tool_control(true);
                backoff.succeeded();
                loop {
                    let notice = tokio::select! {
                        ()=self.inner.shutdown.cancelled()=>return Ok::<_,DbError>(()),
                        notice=notices.next()=>notice?,
                    };
                    match notice {
                        orch_db::sequences::Notice::Reconnected => {
                            self.publish_tool_control(false);
                            self.read_tool_control().await?;
                            self.publish_tool_control(true);
                        }
                        orch_db::sequences::Notice::Heard { payload, .. } => {
                            if payload == "control" {
                                self.read_tool_control().await?;
                                self.publish_tool_control(true);
                            } else if let Ok(segment) = payload.parse::<Id>() {
                                self.publish_tool_session(segment).await?;
                            } else if let Ok(update) =
                                serde_json::from_str::<ToolSessionUpdate>(&payload)
                            {
                                self.broadcast_tool_session(update);
                            }
                        }
                    }
                }
            }
            .await;
            self.publish_tool_control(false);
            if self.inner.shutdown.is_cancelled() {
                break;
            }
            if let Err(error) = result {
                tracing::warn!(%error,"tool session control will reconnect before admitting channels");
            }
            tokio::select! {()=self.inner.shutdown.cancelled()=>break,()=tokio::time::sleep(backoff.failed(None))=>{}}
        }
    }
}
