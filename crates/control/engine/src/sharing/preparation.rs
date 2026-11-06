//! Current share preparation, reused by the guarded writer and rollback-only preview.
use super::*;

/// Server-prepared ordinary share operands. Never deserialized from a client or persisted as authority.
#[derive(Clone)]
pub struct PreparedAgentShare {
    /// Authoritative source, as the existing ordinary source checker found it.
    pub found: Found,
    /// Existing normalized area selection.
    pub parts: ShareParts,
    /// Actual typed recipients, in the ordinary deduplicated order.
    pub recipients: Vec<(Id, ObjectKind)>,
    /// Whether the source name may be included in new human history.
    pub source_name_visible: bool,
    /// Prepared source copy bytes and exact source facts, if a new snapshot is needed.
    pub copies: Option<crate::copies::PreparedCopy>,
    /// The guarded transport already resolved every actual new copy; fallback reads are forbidden.
    pub checked_copy: bool,
    /// Recipient-specific reference rendering, bounded before optional name lookup.
    pub writings: BTreeMap<Id, crate::copies::PreparedWritings>,
    pub(super) destinations: Vec<Destination>,
}
impl PreparedAgentShare {
    /// Current recipients not already covered by this snapshot request's deterministic IDs.
    pub async fn missing_copy_recipients(
        &self,
        conn: &mut Conn,
        request: &ShareWithAgents,
    ) -> Result<Vec<Id>, ShareError> {
        let mut missing = Vec::new();
        for (to, _) in &self.recipients {
            let id = copy_id(&request.client_id, *to, self.found.kind, self.found.object);
            if rows::copy(conn, id).await?.is_none() {
                missing.push(*to);
            }
        }
        Ok(missing)
    }
}

impl Engine {
    /// The unchanged source-View check for a full response after a guarded share commits.
    pub async fn check_agent_share_source_in(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        origin: &Origin,
        subject: Subject,
    ) -> Result<(), ShareError> {
        self.find_subject(conn, principal, origin, principal.now().get(), subject)
            .await
            .map(|_| ())
    }

    /// The existing exact selector, including a false first-reading request that clears stored true.
    /// Only retained flags/identities are inspected here; no names or content are projected.
    pub async fn agent_share_selection(
        &self,
        conn: &mut Conn,
        caller: Id,
        now: DateTime<Utc>,
        request: &ShareWithAgents,
    ) -> Result<&'static str, ShareError> {
        if request.mode == ShareMode::Snapshot {
            return Ok(
                if request
                    .comment
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty())
                {
                    "ShareAgentCreate.snapshot_comment"
                } else {
                    "ShareAgentCreate.snapshot"
                },
            );
        }
        let mut control = control_requested(request);
        if request.read_first == Some(false) {
            let (kind, object, subject, place_kind, people_chat) = match &request.what {
                ShareWhat::Node(n) => {
                    let row = rows::places(conn, &[n.node_id])
                        .await?
                        .into_iter()
                        .next()
                        .ok_or(ShareError::NotFound)?;
                    ("node", row.id, Subject::Node(row.id), Some(row.kind), false)
                }
                ShareWhat::Document(d) => (
                    "document",
                    d.document_id,
                    Subject::Document(d.document_id),
                    None,
                    false,
                ),
                ShareWhat::Chat(c) => {
                    let row = orch_db::queries::chats::chat(conn, c.chat_id)
                        .await?
                        .ok_or(ShareError::NotFound)?;
                    let people = row.is_people_chat();
                    (
                        if people { "chat" } else { "node" },
                        row.node_id.unwrap_or(row.id),
                        Subject::Chat(row.id),
                        None,
                        people,
                    )
                }
            };
            let wanted: Vec<String> = SHARE_PERMISSIONS
                .iter()
                .map(|p| p.as_str().to_owned())
                .collect();
            let records = live_records_of(
                rows::grants_on(conn, kind, object, &wanted, now).await?,
                subject,
                people_chat,
            );
            let ids: Vec<Id> = match &request.to {
                ShareTo::Threads(to) => to.iter().map(|to| to.thread_id).collect(),
                ShareTo::Nodes(to) => to.iter().map(|to| to.node_id).collect(),
            };
            control |= selected_live_records(
                records,
                subject,
                place_kind.as_deref(),
                &ids,
                caller,
                request.inside.unwrap_or(true),
            )
            .iter()
            .any(|record| record.read_first);
        }
        Ok(match (control, request.viewers_read == Some(true)) {
            (false, false) => "ShareAgentCreate.live",
            (false, true) => "ShareAgentCreate.live_viewers_read",
            (true, false) => "ShareAgentCreate.live_read_first",
            (true, true) => "ShareAgentCreate.live_read_first_viewers_read",
        })
    }

    /// The ordinary effect-free source/area/recipient checks shared by both actual and preview writers.
    pub async fn prepare_agent_share_in(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        origin: &Origin,
        request: &ShareWithAgents,
    ) -> Result<PreparedAgentShare, ShareError> {
        if request.client_id.trim().is_empty() {
            return Err(invalid(
                "client_id",
                "is the value that makes asking again safe",
            ));
        }
        if request.mode == ShareMode::Live
            && request
                .comment
                .as_ref()
                .is_some_and(|text| !text.trim().is_empty())
        {
            return Err(invalid(
                "comment",
                "an accompanying input is supported for a copy, with its separate control checks",
            ));
        }
        let now = principal.now().get();
        let asked = match &request.what {
            ShareWhat::Node(node) => Subject::Node(node.node_id),
            ShareWhat::Chat(chat) => Subject::Chat(chat.chat_id),
            ShareWhat::Document(doc) => Subject::Document(doc.document_id),
        };
        let (found, loaded) = self
            .find_subject(conn, principal, origin, now, asked)
            .await?;
        let parts = parts_asked(
            &found,
            request.parts,
            request.mode,
            request.inside.unwrap_or(true),
        )?;
        let selection = self
            .agent_share_selection(conn, principal.user(), now, request)
            .await?;
        let control = control_requested(request) || selection.contains("live_read_first");
        let destinations = self
            .destinations(conn, principal, &request.to, request.mode, control)
            .await?;
        if request.expires_at.is_some_and(|end| end.get() <= now) {
            return Err(invalid("expires_at", "is already past"));
        }
        if request.read_first == Some(true) && parts.documents.is_none() {
            return Err(invalid("read_first", "requires included document material"));
        }
        if request.viewers_read == Some(true)
            && (request.mode != ShareMode::Live
                || parts.documents.is_none()
                || destinations.iter().any(|to| !to.is_thread()))
        {
            return Err(invalid(
                "viewers_read",
                "requires live document material and individual recipient threads",
            ));
        }
        if request.mode == ShareMode::Live
            && found.people_chat
            && loaded.as_ref().is_some_and(|chat| !chat.standing.in_chat)
        {
            return Err(ShareError::NotFound);
        }
        let recipients = destinations
            .iter()
            .map(|to| {
                let kind = match to.place.kind.as_str() {
                    "org" => ObjectKind::Org,
                    "project" => ObjectKind::Project,
                    "topic" => ObjectKind::Topic,
                    "thread" => ObjectKind::Thread,
                    _ => return Err(ShareError::NotFound),
                };
                Ok((to.id, kind))
            })
            .collect::<Result<Vec<_>, ShareError>>()?;
        Ok(PreparedAgentShare {
            found,
            parts,
            recipients,
            source_name_visible: true,
            copies: None,
            checked_copy: false,
            writings: BTreeMap::new(),
            destinations,
        })
    }
}
