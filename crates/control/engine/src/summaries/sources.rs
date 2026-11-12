//! The actual native Read sources of resource summaries, before capability/model admission.
//!
//! This is effect-free preparation. Its returned facts are not authority: the consumer binds
//! the complete footprint through the common operation gate, captures that exact act, and
//! repeats current source/credential/provider guards before IO and publication.
use crate::{
    EngineError,
    operation_scope::{self, targets},
};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyAction, KeyResolvedTarget},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    operations::FacetUsage,
    summaries::{SummaryChatRange, SummaryTarget},
};
use orch_authz::{Act, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{chats, documents, hierarchy, summaries as rows, threads},
};
use orch_summarizer_contract::activity::{ActivityItem, ActivityKind};
use orch_summarizer_model::context::{self, InputItem};
pub use orch_summarizer_model::context::{Budget, Plan};
use orch_types::{Hash, Id, Json, Time};
use serde_json::json;

/// Same metadata page size as the ordinary node visibility collector. This bounds temporary
/// graph/frame work; hidden/private candidates never consume a producer's token allocation.
pub(super) const SOURCE_PAGE: i64 = 100;

/// Source preparation refuses without projecting names, private content, or raw query errors.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// Missing, unreadable or unsupported actual target/source.
    #[error("the summary source is unavailable")]
    Unavailable,
    /// A catch-up must be an ordered nonnegative range inside this actual chat.
    #[error("the chat summary range is invalid")]
    Range,
    /// Native source query failure.
    #[error(transparent)]
    Storage(#[from] DbError),
    /// The ordinary access engine's outcome, preserved for its transport adapter.
    #[error(transparent)]
    Authz(#[from] orch_authz::Error),
    /// The native chat reader's outcome; disclosure still needs its once-only inspection audit.
    #[error(transparent)]
    Chat(#[from] orch_chat::Error),
    /// Canonical operation declaration/binding failure.
    #[error(transparent)]
    Scope(#[from] operation_scope::BindingError),
    /// Existing masking/history producer failure.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl From<orch_authz::key_targets::TargetError> for SourceError {
    fn from(error: orch_authz::key_targets::TargetError) -> Self {
        use orch_authz::key_targets::TargetError;
        match error {
            TargetError::Missing => Self::Unavailable,
            TargetError::Db(error) => Self::Storage(error),
            TargetError::Authz(error) => Self::Authz(error),
        }
    }
}

/// The consuming operation's actual canonical Read roles. No independent key evaluator.
/// Construction requires Read roles; a Write/Use role cannot stand in for source Read.
pub struct ReadScope<'a> {
    plan: &'a operation_scope::Prepared,
    primary: usize,
    inputs: usize,
}
impl<'a> ReadScope<'a> {
    /// Role indices come from the trusted operation's own source declaration, never JSON.
    pub fn new(
        plan: &'a operation_scope::Prepared,
        primary: usize,
        inputs: usize,
    ) -> Result<Self, SourceError> {
        for index in [primary, inputs] {
            if !plan.variant().facets.get(index).is_some_and(|facet| {
                facet.action == KeyAction::Read && facet.usage == FacetUsage::Required
            }) {
                return Err(SourceError::Unavailable);
            }
        }
        Ok(Self {
            plan,
            primary,
            inputs,
        })
    }
    fn allows(
        &self,
        principal: &Principal,
        target: &KeyResolvedTarget,
        primary: bool,
    ) -> Result<bool, SourceError> {
        Ok(self.plan.permits_binding(
            principal.caller(),
            if primary { self.primary } else { self.inputs },
            target.object.kind,
            std::slice::from_ref(target),
        )?)
    }
}

/// Current factual resource identity/placement. No content getter or serialized authority.
pub struct Identity {
    target: SummaryTarget,
    place: Id,
    fact: KeyResolvedTarget,
    title: String,
    version: Json,
}

// Private freshness data only. The transient KeyResolvedTarget deliberately has no wire
// representation; this snapshot never restores it or grants source/operation admission.
fn placement_snapshot(fact: &KeyResolvedTarget) -> Json {
    json!({
        "object": fact.object,
        "node_ancestors": fact.node_ancestors,
        "document_ancestors": fact.document_ancestors,
    })
}
impl Identity {
    /// Actual target requested.
    #[must_use]
    pub fn target(&self) -> SummaryTarget {
        self.target
    }
    /// Actual governing node used for settings, redaction and shared caps.
    #[must_use]
    pub fn place(&self) -> Id {
        self.place
    }
    /// Actual native source fact, for the consuming operation's canonical binder.
    #[must_use]
    pub fn fact(&self) -> &KeyResolvedTarget {
        &self.fact
    }
}

/// Resolve the actual native source before ordinary Read and the complete scope gate. A
/// document resolves as Document with its real folder ancestry; a chat keeps its Chat family.
pub async fn identify(conn: &mut Conn, target: SummaryTarget) -> Result<Identity, SourceError> {
    let (place, title, version, kind) = match target {
        SummaryTarget::Thread { id } => {
            let row = threads::thread(conn, id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(SourceError::Unavailable)?;
            let node = hierarchy::node(conn, id)
                .await?
                .filter(|row| row.kind == "thread")
                .ok_or(SourceError::Unavailable)?;
            (
                id,
                row.name,
                json!({"node_revision":node.revision,"continuation":row.active_continuation_id,
                "execution_revision":row.execution_revision,"activity_seq":row.activity_seq}),
                ObjectKind::Thread,
            )
        }
        SummaryTarget::Topic { id } => {
            let row = hierarchy::node(conn, id)
                .await?
                .filter(|row| row.kind == "topic")
                .ok_or(SourceError::Unavailable)?;
            (
                id,
                row.name,
                json!({"node_revision":row.revision}),
                ObjectKind::Topic,
            )
        }
        SummaryTarget::Document { id } => {
            let row = documents::get(conn, id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(SourceError::Unavailable)?;
            (
                row.node_id,
                row.title,
                json!({"doc_seq":row.doc_seq}),
                ObjectKind::Document,
            )
        }
        SummaryTarget::Chat { id } | SummaryTarget::ChatTitle { id } => {
            let row = chats::chat(conn, id)
                .await?
                .ok_or(SourceError::Unavailable)?;
            let fact = targets::from_chat(conn, &row).await?;
            return Ok(Identity {
                target,
                place: row.home(),
                fact,
                title: row.name.unwrap_or_default(),
                version: json!({"seq_state":row.seq_state}),
            });
        }
    };
    let fact = targets::page_object(
        conn,
        ObjectRef {
            kind,
            id: target.id(),
        },
    )
    .await?;
    Ok(Identity {
        target,
        place,
        fact,
        title,
        version,
    })
}

/// Current ordinary source Read. The consuming operation must also bind its canonical scope.
pub async fn readable(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
) -> Result<bool, SourceError> {
    accessible(conn, principal, target, ReadKind::Model).await
}

/// Passive stored-content Read, including the native inspection standing. The transport
/// must note every inspected chat through the ordinary audited opening before publication.
/// This does not admit a source for a new external-model request.
pub async fn viewable(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
) -> Result<bool, SourceError> {
    accessible(conn, principal, target, ReadKind::Stored).await
}

#[derive(Clone, Copy)]
enum ReadKind {
    Model,
    Stored,
}

async fn accessible(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    kind: ReadKind,
) -> Result<bool, SourceError> {
    Ok(match target {
        SummaryTarget::Thread { id } | SummaryTarget::Topic { id } => {
            let permission = if matches!(target, SummaryTarget::Thread { .. }) {
                Permission::ThreadView
            } else {
                Permission::TopicView
            };
            orch_authz::nodes::check(conn, principal, id, permission, Act::Use, Policy::default())
                .await?
                .is_allowed()
        }
        SummaryTarget::Document { id } => orch_authz::documents::check(
            conn,
            principal,
            id,
            Permission::DocView,
            Policy::default(),
        )
        .await?
        .is_allowed(),
        SummaryTarget::Chat { id } | SummaryTarget::ChatTitle { id } => {
            let origin = orch_db::audit::current_origin();
            let act = orch_chat::Acting {
                principal,
                now: principal.now().get(),
                origin: &origin,
            };
            match orch_chat::chats::load(conn, &act, id, orch_chat::chats::Mode::Read).await {
                Ok(loaded) => matches!(kind, ReadKind::Stored) || !loaded.standing.inspects(),
                Err(orch_chat::Error::NotFound | orch_chat::Error::Lacks { .. }) => false,
                Err(error) => return Err(error.into()),
            }
        }
    })
}

fn scoped(
    scope: Option<&ReadScope<'_>>,
    principal: &Principal,
    fact: &KeyResolvedTarget,
    primary: bool,
) -> Result<bool, SourceError> {
    match scope {
        Some(scope) => scope.allows(principal, fact, primary),
        // Standing automatic writers have no incoming credential to borrow. An explicit key
        // must supply the actual operation roles; it cannot opt out of its original ceiling.
        None => Ok(principal.caller().grant().is_none()
            && principal.caller().native_machine().is_none()
            && !principal.caller().as_admin()),
    }
}

/// Pin the native reader cursor without advancing it. Keep this result with the admitted
/// request before a chat page performs its independent ordinary mark-read act.
pub async fn catch_up_range(
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
) -> Result<SummaryChatRange, SourceError> {
    capture_range(conn, principal, chat, ReadKind::Model).await
}

/// Pins a passive reader's range without creating membership, state, a read mark or work.
pub async fn disclosure_range(
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
) -> Result<SummaryChatRange, SourceError> {
    capture_range(conn, principal, chat, ReadKind::Stored).await
}

async fn capture_range(
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
    kind: ReadKind,
) -> Result<SummaryChatRange, SourceError> {
    if !accessible(conn, principal, SummaryTarget::Chat { id: chat }, kind).await? {
        return Err(SourceError::Unavailable);
    }
    let row = chats::chat(conn, chat)
        .await?
        .ok_or(SourceError::Unavailable)?;
    let after = chats::state(conn, principal.user(), chat)
        .await?
        .map_or(0, |state| state.last_read_seq);
    Ok(SummaryChatRange {
        after: after.min(row.last_message_seq).max(0),
        through: row.last_message_seq,
    })
}

/// Redacted bounded material and complete actual Read facts, not a reusable admission token.
pub struct Material {
    /// Current native primary identity.
    pub identity: Identity,
    /// Current readable title supplied to the normal paragraph prompt.
    pub title: String,
    /// Conformed redacted source items, in reading order.
    pub activity: Vec<ActivityItem>,
    /// Additional actual source identities, such as readable unrestricted topic children.
    pub inputs: Vec<KeyResolvedTarget>,
    /// Every governing resource; later paragraph/model-input reads must recheck these.
    pub source_refs: Vec<SummaryTarget>,
    /// Current material/placement fingerprint; private persistence only.
    pub snapshot: Json,
    /// A chat's immutable catch-up selection, independently of later read marks/new messages.
    pub range: Option<SummaryChatRange>,
    /// Whether every eligible source item and its full text fit this input allowance.
    pub complete: bool,
}

/// Select actual ordinary readable material. No cache, mark, queue or model write occurs.
pub async fn materialize(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    scope: Option<&ReadScope<'_>>,
    range: Option<SummaryChatRange>,
    budget: Budget,
) -> Result<Material, SourceError> {
    materialize_with_plan(conn, principal, target, scope, range, budget, None).await
}

/// Repeat factual cut positions against current native words, never stored prompt text.
pub async fn materialize_with_plan(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    scope: Option<&ReadScope<'_>>,
    range: Option<SummaryChatRange>,
    budget: Budget,
    previous: Option<Plan>,
) -> Result<Material, SourceError> {
    collect(
        conn,
        principal,
        target,
        scope,
        range,
        budget,
        Mode {
            kind: ReadKind::Model,
            latest: false,
            previous,
        },
    )
    .await
}

/// Retained cut positions contain no words or authority; every use re-reads current native sources.
pub fn context_plan(snapshot: &Json) -> Result<Option<Plan>, SourceError> {
    snapshot
        .get("context")
        .and_then(|context| context.get("plan"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(EngineError::from)
        .map_err(SourceError::from)
}

/// Effect-free freshness selection for an already stored paragraph, including passive inspection.
pub async fn disclosure_material(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    scope: &ReadScope<'_>,
    range: Option<SummaryChatRange>,
    budget: Budget,
    previous: Option<Plan>,
) -> Result<Material, SourceError> {
    collect(
        conn,
        principal,
        target,
        Some(scope),
        range,
        budget,
        Mode {
            kind: ReadKind::Stored,
            latest: false,
            previous,
        },
    )
    .await
}

struct Mode {
    kind: ReadKind,
    latest: bool,
    previous: Option<Plan>,
}
struct Reading<'a> {
    principal: &'a Principal,
    identity: &'a Identity,
    scope: Option<&'a ReadScope<'a>>,
    budget: Budget,
    kind: ReadKind,
    latest: bool,
    previous: Option<&'a Plan>,
}
struct Collected {
    items: Vec<InputItem>,
    total: u64,
    versions: Vec<Json>,
    inputs: Vec<KeyResolvedTarget>,
    refs: Vec<SummaryTarget>,
    range: Option<SummaryChatRange>,
}
impl Collected {
    fn new(target: SummaryTarget) -> Self {
        Self {
            items: vec![],
            total: 0,
            versions: vec![],
            inputs: vec![],
            refs: vec![target],
            range: None,
        }
    }
}
struct Window {
    items: Vec<InputItem>,
    budget: Budget,
    minimum: u64,
    oldest: Option<String>,
    oldest_seen: bool,
    latest: bool,
}
impl Window {
    fn new(reading: &Reading<'_>) -> Self {
        let oldest = reading
            .previous
            .filter(|plan| plan.budget == reading.budget)
            .and_then(Plan::oldest_key)
            .map(str::to_owned);
        Self {
            items: vec![],
            budget: reading.budget,
            minimum: 0,
            oldest_seen: oldest.is_none(),
            oldest,
            latest: reading.latest,
        }
    }
    fn push(&mut self, mut input: InputItem) -> bool {
        if self.oldest.as_deref() == Some(input.key.as_str()) {
            self.oldest_seen = true;
        }
        if input.item.text.trim().is_empty() {
            return false;
        }
        input.item.text = input.item.text.trim().to_owned();
        self.minimum = self
            .minimum
            .saturating_add(u64::from(self.budget.detail(&input.item)));
        self.items.push(input);
        // A reusable old window plus admissible new material weighs at most two activity budgets.
        self.latest
            || self.minimum > u64::from(self.budget.available())
                && (self.oldest_seen
                    || self.minimum > u64::from(self.budget.available()).saturating_mul(2))
    }
    fn finish(mut self) -> Vec<InputItem> {
        self.items.reverse();
        self.items
    }
}

async fn mask_inputs(
    conn: &mut Conn,
    identity: &Identity,
    items: &mut [InputItem],
) -> Result<String, SourceError> {
    let place_thread = threads::thread(conn, identity.place).await?;
    let topic = place_thread
        .as_ref()
        .map_or(identity.place, |row| row.topic_id);
    let environment = place_thread.as_ref().and_then(|row| row.environment_id);
    let texts: Vec<_> = items.iter().map(|input| input.item.text.as_str()).collect();
    let raw = json!({"title":identity.title,"activity":texts});
    let redacted = crate::raw_events::mask_summary(conn, identity.place, topic, environment, raw)
        .await?
        .ok_or(SourceError::Unavailable)?;
    let title = redacted["title"]
        .as_str()
        .ok_or(SourceError::Unavailable)?
        .to_owned();
    let texts = redacted["activity"]
        .as_array()
        .filter(|texts| texts.len() == items.len())
        .ok_or(SourceError::Unavailable)?;
    for (input, text) in items.iter_mut().zip(texts) {
        input.item.text = text.as_str().ok_or(SourceError::Unavailable)?.to_owned();
    }
    Ok(title)
}

async fn thread_input(
    conn: &mut Conn,
    reading: &Reading<'_>,
    id: Id,
) -> Result<Collected, SourceError> {
    let row = threads::thread(conn, id)
        .await?
        .ok_or(SourceError::Unavailable)?;
    let mut out = Collected::new(reading.identity.target);
    out.total = u64::try_from(
        orch_db::queries::thread_history::summary_activity_total(
            conn,
            id,
            row.active_continuation_id,
            0,
        )
        .await?,
    )
    .unwrap_or(0);
    let mut window = Window::new(reading);
    let mut before = None;
    'pages: loop {
        let page = orch_db::queries::thread_history::summary_activity_page(
            conn,
            id,
            row.active_continuation_id,
            0,
            before,
            SOURCE_PAGE,
        )
        .await?;
        let length = page.len();
        let mut batch = Vec::new();
        for source in page {
            before = Some(source.seq_created);
            let item = super::masked_activity(conn, &row, &source.kind, &source.payload, source.at)
                .await
                .map_err(|error| {
                    SourceError::Engine(EngineError::unreadable(super::safe_reason(&error)))
                })?;
            batch.push(InputItem {
                key: format!("thread:{id}:{}", source.seq_created),
                item,
            });
        }
        mask_inputs(conn, reading.identity, &mut batch).await?;
        for input in batch {
            if window.push(input) {
                break 'pages;
            }
        }
        if length < usize::try_from(SOURCE_PAGE).unwrap_or(usize::MAX) {
            break;
        }
    }
    out.items = window.finish();
    Ok(out)
}

async fn document_input(
    conn: &mut Conn,
    reading: &Reading<'_>,
    id: Id,
) -> Result<Collected, SourceError> {
    let mut out = Collected::new(reading.identity.target);
    out.total = u64::try_from(rows::document_text_total(conn, id).await?).unwrap_or(0);
    let mut before = None;
    let mut window = Window::new(reading);
    'pages: loop {
        let page = rows::document_text(conn, id, before, SOURCE_PAGE).await?;
        let length = page.len();
        if let Some(last) = page.last() {
            before = Some((last.ordinal, last.id));
        }
        let mut batch = Vec::new();
        for source in page {
            out.versions
                .push(json!({"id":source.id,"seq_updated":source.seq_updated}));
            batch.push(InputItem {
                key: format!("document:{}", source.id),
                item: ActivityItem {
                    kind: ActivityKind::Document,
                    text: source.text,
                    at: Some(Time::new(source.at)),
                },
            });
        }
        mask_inputs(conn, reading.identity, &mut batch).await?;
        for input in batch {
            if window.push(input) {
                break 'pages;
            }
        }
        if length < usize::try_from(SOURCE_PAGE).unwrap_or(usize::MAX) {
            break;
        }
    }
    out.items = window.finish();
    Ok(out)
}

async fn chat_input(
    conn: &mut Conn,
    reading: &Reading<'_>,
    id: Id,
    range: Option<SummaryChatRange>,
) -> Result<Collected, SourceError> {
    let row = chats::chat(conn, id)
        .await?
        .ok_or(SourceError::Unavailable)?;
    let range = match range {
        Some(range) => range,
        None if matches!(reading.identity.target, SummaryTarget::ChatTitle { .. }) => {
            SummaryChatRange {
                after: 0,
                through: row.last_message_seq,
            }
        }
        None => capture_range(conn, reading.principal, id, reading.kind).await?,
    };
    if range.after < 0 || range.through < range.after || range.through > row.last_message_seq {
        return Err(SourceError::Range);
    }
    let mut out = Collected::new(reading.identity.target);
    out.range = Some(range);
    out.total = u64::try_from(rows::chat_text_total(conn, id, range.after, range.through).await?)
        .unwrap_or(0);
    let mut before = None;
    let mut window = Window::new(reading);
    'pages: loop {
        let page =
            rows::chat_text(conn, id, range.after, range.through, before, SOURCE_PAGE).await?;
        let length = page.len();
        if let Some(last) = page.last() {
            before = Some(last.ordinal);
        }
        let mut batch = Vec::new();
        for source in page {
            out.versions
                .push(json!({"id":source.id,"seq_updated":source.seq_updated}));
            batch.push(InputItem {
                key: format!("chat:{}", source.id),
                item: ActivityItem {
                    kind: ActivityKind::UserMessage,
                    text: source.text,
                    at: Some(Time::new(source.at)),
                },
            });
        }
        mask_inputs(conn, reading.identity, &mut batch).await?;
        for input in batch {
            if window.push(input) {
                break 'pages;
            }
        }
        if length < usize::try_from(SOURCE_PAGE).unwrap_or(usize::MAX) {
            break;
        }
    }
    out.items = window.finish();
    Ok(out)
}

async fn topic_input(
    conn: &mut Conn,
    reading: &Reading<'_>,
    id: Id,
) -> Result<Collected, SourceError> {
    let mut out = Collected::new(reading.identity.target);
    let mut after: Option<(chrono::DateTime<chrono::Utc>, Id, String)> = None;
    let mut window = Window::new(reading);
    let mut full = false;
    loop {
        let page = rows::topic_sources(
            conn,
            id,
            after
                .as_ref()
                .map(|(at, id, kind)| (*at, *id, kind.as_str())),
            SOURCE_PAGE,
        )
        .await?;
        let places = page.iter().map(|source| source.place).collect::<Vec<_>>();
        let private = orch_authz::nodes::restricted_below(
            conn,
            reading.principal,
            &places,
            id,
            Policy::default(),
        )
        .await?;
        for child in &page {
            after = Some((child.at, child.id, child.kind.clone()));
            if private.contains(&child.place) {
                continue;
            }
            let child_target = match child.kind.as_str() {
                "thread" => SummaryTarget::Thread { id: child.id },
                "chat" => SummaryTarget::Chat { id: child.id },
                _ => return Err(SourceError::Unavailable),
            };
            let child_identity = identify(conn, child_target).await?;
            if !scoped(
                reading.scope,
                reading.principal,
                child_identity.fact(),
                false,
            )? || !accessible(conn, reading.principal, child_target, reading.kind).await?
            {
                continue;
            }
            if let SummaryTarget::Chat { id: chat } = child_target {
                let row = chats::chat(conn, chat)
                    .await?
                    .ok_or(SourceError::Unavailable)?;
                if rows::chat_text_total(conn, chat, 0, row.last_message_seq).await? == 0 {
                    continue;
                }
            }
            out.total = out.total.saturating_add(1);
            out.versions.push(json!({"target":child_target,"placement":placement_snapshot(child_identity.fact()),"version":child_identity.version}));
            out.inputs.push(child_identity.fact().clone());
            out.refs.push(child_target);
            if full {
                continue;
            }
            let detail = match child_target {
                SummaryTarget::Thread { id: thread } => {
                    let row = threads::thread(conn, thread)
                        .await?
                        .ok_or(SourceError::Unavailable)?;
                    let latest = orch_db::queries::thread_history::summary_activity_page(
                        conn,
                        thread,
                        row.active_continuation_id,
                        0,
                        None,
                        1,
                    )
                    .await?;
                    let text = if let Some(source) = latest.first() {
                        super::masked_activity(conn, &row, &source.kind, &source.payload, source.at)
                            .await
                            .map_err(|error| {
                                SourceError::Engine(EngineError::unreadable(super::safe_reason(
                                    &error,
                                )))
                            })?
                            .text
                    } else {
                        String::new()
                    };
                    let title = crate::raw_events::mask_live(
                        conn,
                        &row,
                        row.environment_id,
                        json!(child_identity.title),
                    )
                    .await?
                    .ok_or(SourceError::Unavailable)?;
                    format!(
                        "{}: {}",
                        title.as_str().ok_or(SourceError::Unavailable)?,
                        text
                    )
                }
                SummaryTarget::Chat { id: chat } => {
                    let row = chats::chat(conn, chat)
                        .await?
                        .ok_or(SourceError::Unavailable)?;
                    let child_scope = reading
                        .scope
                        .map(|scope| ReadScope::new(scope.plan, scope.inputs, scope.inputs))
                        .transpose()?;
                    let material = Box::pin(collect(
                        conn,
                        reading.principal,
                        child_target,
                        child_scope.as_ref(),
                        Some(SummaryChatRange {
                            after: 0,
                            through: row.last_message_seq,
                        }),
                        reading.budget,
                        Mode {
                            kind: reading.kind,
                            latest: true,
                            previous: None,
                        },
                    ))
                    .await?;
                    let Some(latest) = material.activity.last() else {
                        continue;
                    };
                    format!(
                        "{}: {}",
                        if material.title.is_empty() {
                            "Chat"
                        } else {
                            &material.title
                        },
                        latest.text
                    )
                }
                _ => return Err(SourceError::Unavailable),
            };
            let mut batch = vec![InputItem {
                key: format!("{}:{}", child.kind, child.id),
                item: ActivityItem {
                    kind: if child.kind == "chat" {
                        ActivityKind::UserMessage
                    } else {
                        ActivityKind::AgentMessage
                    },
                    text: detail,
                    at: Some(Time::new(child.at)),
                },
            }];
            mask_inputs(conn, reading.identity, &mut batch).await?;
            for input in batch {
                full = window.push(input);
            }
        }
        // Keep counting only authorized metadata after filling; omitted sources govern the disclosed count too.
        let done = page.len() < usize::try_from(SOURCE_PAGE).unwrap_or(usize::MAX);
        if done {
            break;
        }
    }
    out.items = window.finish();
    Ok(out)
}

async fn collect(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    scope: Option<&ReadScope<'_>>,
    range: Option<SummaryChatRange>,
    budget: Budget,
    mut mode: Mode,
) -> Result<Material, SourceError> {
    let identity = identify(conn, target).await?;
    if !scoped(scope, principal, identity.fact(), true)?
        || !accessible(conn, principal, target, mode.kind).await?
    {
        return Err(SourceError::Unavailable);
    }
    if range.is_some()
        && !matches!(
            target,
            SummaryTarget::Chat { .. } | SummaryTarget::ChatTitle { .. }
        )
    {
        return Err(SourceError::Range);
    }
    Budget::new(budget.tokens, budget.frame).map_err(|error| {
        SourceError::Engine(EngineError::unreadable(super::safe_reason(&error)))
    })?;
    if mode.previous.is_none() && !mode.latest {
        let own = rows::get(conn, target, Some(principal.user())).await?;
        if let Some(snapshot) = own
            .as_ref()
            .and_then(|cache| cache.source_snapshot.as_ref())
        {
            mode.previous = context_plan(snapshot)?;
        }
        if mode.previous.is_none() {
            let shared = rows::get(conn, target, None).await?;
            if let Some(snapshot) = shared
                .as_ref()
                .and_then(|cache| cache.source_snapshot.as_ref())
            {
                mode.previous = context_plan(snapshot)?;
            }
        }
    }
    let reading = Reading {
        principal,
        identity: &identity,
        scope,
        budget,
        kind: mode.kind,
        latest: mode.latest,
        previous: mode.previous.as_ref(),
    };
    let collected = match target {
        SummaryTarget::Thread { id } => Box::pin(thread_input(conn, &reading, id)).await?,
        SummaryTarget::Topic { id } => Box::pin(topic_input(conn, &reading, id)).await?,
        SummaryTarget::Document { id } => Box::pin(document_input(conn, &reading, id)).await?,
        SummaryTarget::Chat { id } | SummaryTarget::ChatTitle { id } => {
            Box::pin(chat_input(conn, &reading, id, range)).await?
        }
    };
    let title = mask_inputs(conn, &identity, &mut []).await?;
    let filled = context::fill(
        collected.items,
        budget,
        if mode.latest {
            None
        } else {
            mode.previous.as_ref()
        },
        collected.total,
    )
    .map_err(|error| SourceError::Engine(EngineError::unreadable(super::safe_reason(&error))))?;
    let complete = filled.facts.omitted_items == 0 && filled.facts.shortened_items == 0;
    let digest = Hash::of(
        &serde_json::to_vec(&json!({"title":title,"activity":filled.activity}))
            .map_err(EngineError::from)?,
    );
    let snapshot = json!({"target":target,"place":identity.place,"placement":placement_snapshot(&identity.fact),"version":identity.version,
        "inputs":collected.versions,"range":collected.range,"complete":complete,"content":digest,"context":{"plan":filled.plan,"facts":filled.facts}});
    Ok(Material {
        identity,
        title,
        activity: filled.activity,
        inputs: collected.inputs,
        source_refs: collected.refs,
        snapshot,
        range: collected.range,
        complete,
    })
}

/// Whether a cached paragraph still represents the same visible words and native identities.
/// Version-only churn in an omitted/private child does not mark a public paragraph stale;
/// effect/publication guards still compare the complete exact source snapshot.
#[must_use]
pub fn same_content(kept: &Json, current: &Json) -> bool {
    let leaves = [
        "target",
        "place",
        "placement",
        "content",
        "range",
        "complete",
        "context",
    ];
    if leaves
        .iter()
        .any(|key| kept.get(key).is_none() || kept.get(key) != current.get(key))
    {
        return false;
    }
    let identities = |snapshot: &Json| {
        snapshot
            .get("inputs")
            .and_then(Json::as_array)
            .map(|inputs| {
                inputs
                    .iter()
                    .map(|input| input.get("target").or_else(|| input.get("id")).cloned())
                    .collect::<Vec<_>>()
            })
    };
    identities(kept) == identities(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_access_contract::key_scope::{KeyObjectRef, KeyTargetKind};

    #[test]
    fn freshness_tracks_actual_node_and_folder_placement_without_serializing_admission() {
        let reference = |kind, id| KeyObjectRef {
            kind: KeyTargetKind::Object(kind),
            id: Some(id),
        };
        let mut fact = KeyResolvedTarget {
            object: reference(ObjectKind::Document, orch_types::new_id()),
            node_ancestors: vec![reference(ObjectKind::Topic, orch_types::new_id())],
            document_ancestors: vec![reference(
                ObjectKind::DocumentContainer,
                orch_types::new_id(),
            )],
            credential_account: None,
        };
        let original = placement_snapshot(&fact);
        fact.node_ancestors[0] = reference(ObjectKind::Topic, orch_types::new_id());
        assert_ne!(original, placement_snapshot(&fact));
        let moved_node = placement_snapshot(&fact);
        fact.document_ancestors[0] = reference(ObjectKind::DocumentContainer, orch_types::new_id());
        assert_ne!(moved_node, placement_snapshot(&fact));
        assert!(original.get("credential_account").is_none());
        assert_eq!(original.as_object().map(serde_json::Map::len), Some(3));
    }
}
