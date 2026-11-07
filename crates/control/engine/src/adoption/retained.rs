//! The actual original marker, discovery and bounded record supplier for history retry.

use super::*;
use crate::harnesses::HarnessKey;
use orch_db::DbError;

/// The retained native source, loaded from the original adopted marker and segment.
/// Its fields are private so a request cannot replace the marker with submitted ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedHistory {
    thread: Id,
    marker: Id,
    segment: Id,
    provider: Id,
    agent: Id,
    topic: Id,
    reference: String,
    before: Time,
    author: Option<Id>,
    observed_at: Time,
    history_missing: bool,
}
impl RetainedHistory {
    /// Actual adopted Thread.
    pub fn thread(&self) -> Id {
        self.thread
    }
    /// Actual original adopted marker.
    pub fn marker(&self) -> Id {
        self.marker
    }
    /// Original segment whose native references deduplicate the imported entries.
    pub fn segment(&self) -> Id {
        self.segment
    }
    /// Actual original Provider.
    pub fn provider(&self) -> Id {
        self.provider
    }
    /// Actual original Agent profile.
    pub fn agent(&self) -> Id {
        self.agent
    }
    /// Topic observed when this retained Thread source was selected.
    pub fn topic(&self) -> Id {
        self.topic
    }
    /// Native session reference from the original marker.
    pub fn native_session_ref(&self) -> &str {
        &self.reference
    }
    /// The original native history cutoff, never the retry's time.
    pub fn before(&self) -> Time {
        self.before
    }
    /// Original historical attribution, never current caller authority.
    pub fn author(&self) -> Option<Id> {
        self.author
    }
    /// Original marker observation time.
    pub fn observed_at(&self) -> Time {
        self.observed_at
    }
    /// Actual marker status at selection; an already completed import needs no new native read.
    pub fn history_missing(&self) -> bool {
        self.history_missing
    }
    pub(super) fn adoption(&self) -> Adoption {
        Adoption {
            agent_access: None,
            topic: self.topic,
            provider: self.provider,
            agent: self.agent,
            native_session_ref: self.reference.clone(),
            environment: None,
            name: None,
        }
    }
    /// Verify the original native source again without formatting or granting any visibility.
    pub async fn current(&self, conn: &mut Conn) -> Result<bool, AdoptError> {
        Ok(retained_history_source(conn, self.thread)
            .await?
            .is_some_and(|current| self.same_source(&current)))
    }
    pub(super) fn same_source(&self, current: &Self) -> bool {
        self.thread == current.thread
            && self.marker == current.marker
            && self.segment == current.segment
            && self.provider == current.provider
            && self.agent == current.agent
            && self.reference == current.reference
            && self.before == current.before
            && self.author == current.author
            && self.observed_at == current.observed_at
    }
}

/// Factual loader shared by retry admission, native setup, command delivery and record reads.
/// Ordinary Thread Manage and declared source Read remain the consuming producer's gates.
pub async fn retained_history_source(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<RetainedHistory>, AdoptError> {
    type Original = (
        Id,
        Id,
        Id,
        Id,
        Id,
        orch_types::Json,
        chrono::DateTime<chrono::Utc>,
    );
    let original:Option<Original>=sqlx::query_as(
        "SELECT i.id,s.id,s.provider_id,s.agent_id,t.topic_id,i.payload,i.created_at
         FROM items i JOIN segments s ON s.id=i.segment_id JOIN threads t ON t.node_id=i.thread_id
         JOIN nodes n ON n.id=t.node_id
         WHERE i.thread_id=$1 AND i.native_ref='adopted' AND i.kind='marker' AND n.deleted_at IS NULL
         ORDER BY i.seq_created LIMIT 1")
        .bind(thread).fetch_optional(conn).await.map_err(DbError::from)?;
    let Some((marker, segment, provider, agent, topic, payload, observed_at)) = original else {
        return Ok(None);
    };
    let reference = payload["data"]["native_session_ref"]
        .as_str()
        .ok_or(AdoptError::NoSession)?
        .to_owned();
    let before = payload["data"]
        .get("history_before")
        .cloned()
        .map(serde_json::from_value::<Time>)
        .transpose()
        .map_err(EngineError::from)?
        .unwrap_or(Time::new(observed_at));
    let author = payload["data"]
        .get("history_author")
        .cloned()
        .map(serde_json::from_value::<Option<Id>>)
        .transpose()
        .map_err(EngineError::from)?
        .flatten();
    Ok(Some(RetainedHistory {
        thread,
        marker,
        segment,
        provider,
        agent,
        topic,
        reference,
        before,
        author,
        observed_at: Time::new(observed_at),
        history_missing: !payload["data"]["history_missing"].is_null(),
    }))
}

/// The existing ordinary manager door, with the actual already loaded current principal.
pub async fn require_retry_history(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
) -> Result<(), AdoptError> {
    match nodes::check(
        conn,
        principal,
        thread,
        Permission::ThreadManage,
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(EngineError::from)?
    {
        Decision::Allowed { .. } => Ok(()),
        Decision::Refused(refusal) => Err(CreateThreadError::Denied(Box::new(refusal)).into()),
    }
}

/// Actual native record status and decoded entries; a source refusal is never this content result.
pub enum HistoryRecord<'a> {
    /// The original bounded parser's entries or genuine unreadable content reason.
    Read {
        record: &'a AgentReadSessionResult,
        entries: Result<&'a [RecordedEntry], &'a str>,
    },
    /// Native adapter reports no record support.
    Unsupported(&'a str),
    /// Native supplier reports a genuine record failure.
    Failed(&'a str),
}

/// Once-only retained discovery/record observation, carrying the same current native source.
pub struct FetchedHistory {
    source: RetainedHistory,
    session: Held,
    machine: Id,
    recorded: Recorded,
    entries: Option<Result<Vec<RecordedEntry>, String>>,
    observed: operation::RecordObservation<operation::Retrying>,
}
impl FetchedHistory {
    /// Actual original marker/segment/provider/reference association.
    pub fn source(&self) -> &RetainedHistory {
        &self.source
    }
    /// The physical machine which actually supplied this discovery and record.
    pub fn machine(&self) -> Id {
        self.machine
    }
    /// The harness's actual native session observation time, used by the original cutoff rule.
    pub fn session_updated_at(&self) -> Option<Time> {
        self.session.updated_at
    }
    /// Read the native result without importing history, creating input or starting a turn.
    pub fn record(&self) -> Result<HistoryRecord<'_>, EngineError> {
        Ok(match &self.recorded {
            Recorded::Read(record) => HistoryRecord::Read {
                record,
                entries: match self.entries.as_ref() {
                    Some(Ok(entries)) => Ok(entries),
                    Some(Err(why)) => Err(why),
                    None => {
                        return Err(EngineError::unreadable(
                            "the retained record has no native decode result",
                        ));
                    }
                },
            },
            Recorded::Unsupported(why) => HistoryRecord::Unsupported(why),
            Recorded::Failed(why) => HistoryRecord::Failed(why),
        })
    }
    /// Same retained source on the actual writer transaction, after IO and before its first effect.
    pub async fn check_in(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        clock: &dyn orch_types::Clock,
    ) -> Result<operation::Checkpoint, EngineError> {
        self.observed.check_in(conn, caller, clock).await
    }
}

impl Engine {
    /// The retained retry's same discovery, setup, command and bounded store/parser producers.
    /// The caller supplies its real admitted retry operation and actual raw credential/HTTP bridge.
    pub async fn fetch_retained_adopted_history(
        &self,
        caller: &Caller,
        credential: operation::HttpCredential,
        source: RetainedHistory,
        transport: Option<std::sync::Arc<dyn operation::Transport>>,
        until: Option<Time>,
    ) -> Result<FetchedHistory, AdoptError> {
        let preparation =
            operation::Preparation::capture_retry(caller, credential, source.clone())?
                .with_transport(transport, until);
        let adoption = source.adoption();
        let (session, machine) = self
            .held_session_with_preparation(&adoption, Some(&preparation))
            .await?;
        let key = HarnessKey {
            provider: source.provider,
            agent: source.agent,
            machine,
        };
        let mut observed =
            operation::RecordObservation::<operation::Retrying>::open(self, &preparation, key)
                .await?;
        let record_preparation = observed.preparation().clone();
        let recorded = self
            .read_session_record_observed(
                machine,
                &adoption,
                true,
                Some(&record_preparation),
                Some(&mut observed),
            )
            .await?;
        let _ = observed.refresh(self).await?;
        let entries = match &recorded {
            Recorded::Read(read) => {
                let result = self.record_entries_from(read, Some(&mut observed)).await;
                let _ = observed.refresh(self).await?;
                Some(match result {
                    Ok(entries) => Ok(entries),
                    Err(RecordReadError::Content(why)) => Err(why),
                    Err(RecordReadError::Source(error)) => return Err(error.into()),
                })
            }
            _ => None,
        };
        Ok(FetchedHistory {
            source,
            session,
            machine,
            recorded,
            entries,
            observed,
        })
    }
}
