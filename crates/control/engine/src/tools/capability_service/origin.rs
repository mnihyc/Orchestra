//! Conjunctive native origins for the shared paid execution path.
use super::*;
use crate::access::ResourceAccess;
use crate::operation_scope::http_command_source::CommandEvidence;

/// Billing facts retained by the actual native producer, never caller or source authority.
/// Automatic work has no person; a personal act retains its originally admitted issuer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaidAttribution {
    /// The actual native governing place used by provider policies and caps.
    pub place: Id,
    /// Original native placement; these ledger facts do not grant access.
    pub org: Id,
    pub project: Id,
    pub topic: Id,
    pub thread: Option<Id>,
    /// Actual (turn, segment), in the ordinary ledger order; absent for standalone work.
    pub conversation: Option<(Id, Id)>,
    pub person: Option<Id>,
    pub purpose: String,
    pub harness_kind: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "source",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum NativeSource {
    Channel(Box<CommandAuthority>),
    HttpSource(Box<CommandEvidence>),
}
impl NativeSource {
    async fn lock(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::Channel(source) => Box::pin(source.lock_native_source(conn)).await,
            Self::HttpSource(source) => source.lock_source(conn).await,
        }
    }
    pub(crate) fn same_origin(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Channel(a), Self::Channel(b)) => a.same_origin(b),
            _ => self == other,
        }
    }
    async fn current(
        &self,
        conn: &mut Conn,
        command: &link::NewCommand<'_>,
        stored: Option<&link::CommandRow>,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<CurrentCallers>, EngineError> {
        match (self, stored) {
            (Self::Channel(source), Some(row)) => {
                Box::pin(source.current_callers(conn, row, clock, access)).await
            }
            (Self::Channel(source), None) => {
                Box::pin(source.current_issue_callers(conn, command, clock, access)).await
            }
            (Self::HttpSource(source), Some(row)) => {
                source.current_callers(conn, row, clock, access).await
            }
            (Self::HttpSource(source), None) => {
                source
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Origin {
    Unbound,
    Tool {
        source: source::Source,
        call: Box<CallContext>,
    },
    Native {
        source: NativeSource,
        attribution: Box<PaidAttribution>,
    },
}

pub(super) struct Origins {
    pub callers: CurrentCallers,
    pub person: Option<Id>,
    pub harness_kind: Option<String>,
    pub params: Option<Json>,
}
impl Origins {
    pub fn callers(&self) -> impl Iterator<Item = &Caller> {
        self.callers
            .iter()
            .filter_map(|(caller, _)| caller.as_ref())
    }
}
impl Origin {
    pub(super) fn place(&self) -> Result<Id, EngineError> {
        match self {
            Self::Tool { call, .. } => Ok(call.thread),
            Self::Native { attribution, .. } => Ok(attribution.place),
            Self::Unbound => Err(EngineError::StaleToolSource),
        }
    }
    pub(super) fn tool(&self) -> Option<(&source::Source, &CallContext)> {
        match self {
            Self::Tool { source, call } => Some((source, call)),
            _ => None,
        }
    }
    pub(super) async fn lock(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::Tool { call, .. } => {
                let _ = threads::lock(conn, call.thread).await?;
                Ok(())
            }
            Self::Native { source, .. } => source.lock(conn).await,
            Self::Unbound => Err(EngineError::StaleToolSource),
        }
    }
    pub(super) async fn current(
        &self,
        conn: &mut Conn,
        command: &link::NewCommand<'_>,
        stored: Option<&link::CommandRow>,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<Origins>, EngineError> {
        match self {
            Self::Unbound => Ok(None),
            Self::Tool {
                source: scope,
                call,
            } => {
                if stored.is_some() {
                    let associated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_tool_commands WHERE command_id=$1 AND thread_id=$2 AND turn_id=$3 AND answer_id=$4)")
                        .bind(command.id).bind(call.thread).bind(call.turn)
                        .bind(crate::ids::tool_result(call.thread, &call.call_ref))
                        .fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
                    if !associated {
                        return Ok(None);
                    }
                }
                scope
                    .scope(Box::pin(async {
                        let origins = source::original_callers(conn, call, clock, access).await?;
                        Ok(Some(Origins {
                            callers: origins
                                .callers()
                                .iter()
                                .map(|caller| {
                                    (
                                        Some(caller.clone()),
                                        caller.grant().and_then(orch_authz::TokenGrant::expires_at),
                                    )
                                })
                                .collect(),
                            person: origins.attribution().user_id,
                            harness_kind: origins.attribution().harness_kind.clone(),
                            params: Some(origins.attribution().params.clone()),
                        }))
                    }))
                    .await
            }
            Self::Native {
                source,
                attribution,
            } => {
                let Some(callers) = source.current(conn, command, stored, clock, access).await?
                else {
                    return Ok(None);
                };
                // The original native producer retains every issuer, approver and automatic writer.
                // A factual billing person may not introduce a caller which that producer never admitted.
                if callers.is_empty()
                    || callers.iter().any(|(caller, _)| caller.is_none())
                    || attribution.person.is_some_and(|person| {
                        !callers.iter().any(|(caller, _)| {
                            caller
                                .as_ref()
                                .is_some_and(|caller| caller.user() == person)
                        })
                    })
                {
                    return Ok(None);
                }
                Ok(Some(Origins {
                    callers,
                    person: attribution.person,
                    harness_kind: attribution.harness_kind.clone(),
                    params: None,
                }))
            }
        }
    }
}

impl ServiceCommand {
    pub(crate) fn bind_native(
        mut self,
        source: NativeSource,
        attribution: PaidAttribution,
    ) -> Self {
        self.origin = Origin::Native {
            source,
            attribution: Box::new(attribution),
        };
        self
    }
    pub(crate) fn same_origin(&self, other: &Self) -> bool {
        let (
            Origin::Native {
                source: a,
                attribution: aa,
            },
            Origin::Native {
                source: b,
                attribution: bb,
            },
        ) = (&self.origin, &other.origin)
        else {
            return self == other;
        };
        aa == bb
            && a.same_origin(b)
            && Self {
                origin: Origin::Unbound,
                ..self.clone()
            } == Self {
                origin: Origin::Unbound,
                ..other.clone()
            }
    }
    /// Factual retained native evidence for the producer's existing receipt lookup, not authority.
    pub(crate) fn native(&self) -> Option<&CommandAuthority> {
        match &self.origin {
            Origin::Native {
                source: NativeSource::Channel(source),
                ..
            } => Some(source),
            _ => None,
        }
    }
}

pub(super) enum Billing {
    Turn(Box<orch_db::usage::ledger::Attribution>),
    Standalone {
        source: orch_db::usage::ledger::SummaryAttribution,
        conversation: Option<(Id, Id)>,
    },
}
impl Billing {
    pub(super) fn source(&self) -> crate::service_usage::ServiceSource<'_> {
        match self {
            Self::Turn(source) => crate::service_usage::ServiceSource::Turn(source),
            Self::Standalone { source, .. } => {
                crate::service_usage::ServiceSource::Standalone(source)
            }
        }
    }
    pub(super) fn model_attribution(
        &self,
        provider: Id,
        entry: Option<Id>,
    ) -> orch_db::usage::ledger::SummaryAttribution {
        let (org_id, project_id) = match self {
            Self::Turn(source) => (source.org_id, source.project_id),
            Self::Standalone { source, .. } => (source.org_id, source.project_id),
        };
        orch_db::usage::ledger::SummaryAttribution {
            org_id,
            project_id,
            topic_id: self.topic(),
            thread_id: self.thread(),
            provider_id: provider,
            entry_id: entry,
            user_id: self.person(),
        }
    }
    pub(super) fn person(&self) -> Option<Id> {
        match self {
            Self::Turn(source) => source.user_id,
            Self::Standalone { source, .. } => source.user_id,
        }
    }
    pub(super) fn topic(&self) -> Id {
        match self {
            Self::Turn(source) => source.topic_id,
            Self::Standalone { source, .. } => source.topic_id,
        }
    }
    pub(super) fn thread(&self) -> Option<Id> {
        match self {
            Self::Turn(source) => Some(source.thread_id),
            Self::Standalone { source, .. } => source.thread_id,
        }
    }
    pub(super) fn conversation(&self) -> Option<(Id, Id)> {
        match self {
            Self::Turn(source) => Some((source.turn_id, source.segment_id)),
            Self::Standalone { conversation, .. } => *conversation,
        }
    }
}
impl Origin {
    pub(super) async fn billing(
        &self,
        conn: &mut Conn,
        provider: Id,
        entry: Option<Id>,
    ) -> Result<Option<Billing>, EngineError> {
        match self {
            Self::Unbound => Ok(None),
            Self::Tool { call, .. } => {
                Ok(orch_db::usage::ledger::attribution_of_turn(conn, call.turn)
                    .await?
                    .map(|source| Billing::Turn(Box::new(source))))
            }
            Self::Native { attribution, .. } => Ok(Some(Billing::Standalone {
                source: orch_db::usage::ledger::SummaryAttribution {
                    org_id: attribution.org,
                    project_id: attribution.project,
                    topic_id: attribution.topic,
                    thread_id: attribution.thread,
                    provider_id: provider,
                    entry_id: entry,
                    user_id: attribution.person,
                },
                conversation: attribution.conversation,
            })),
        }
    }
}
