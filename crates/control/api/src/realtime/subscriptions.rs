//! Authorization-aware feed subscriptions.
//!
//! Following a feed is an access decision, and the access engine is the only
//! one that makes it: this module asks it when a connection subscribes, and
//! asks it again when the authorization epoch moves or a relevant grant ends.
//! A person who lost what a feed needs is sent `closed` with
//! the reason `revoked` before anything further is sent on it; one whose
//! object is gone, with `deleted`. The same check is made before the rows of a
//! feed are read for a reader whose last check is older than the epoch or time, so
//! that no row is sent on the strength of a decision the engine has since
//! changed.
//!
//! A subscription also keeps what the server needs to send it rows: how far
//! it has read and sent, behind a lock that serializes everything done for it.

use crate::realtime::deltas::Tag;
use crate::realtime::server::Connection;
use crate::realtime::sync::{Cache, Head};
use crate::realtime::{Fail, Inner, Source, chats, documents, threads, topics, tree, watches};
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::{NodePermission, PermissionName};
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::Conn;
use orch_realtime_contract::feeds::Feed;
use orch_realtime_contract::frames::{
    ClosedFrame, FeedClosedReason, PresenceFrame, ResyncFrame, ServerFrame, SocketErrorCode,
    SubFrame, SubId, SubscribedFrame, UnsubFrame,
};
use orch_realtime_contract::pacing::RateLimit;
use orch_realtime_contract::sync::{SendCursor, SubscribeMode};
use orch_realtime_contract::typing::send_limit;
use orch_realtime_contract::version::{MAX_SUBSCRIPTIONS, subscription_allowed};
use orch_types::{Clock, Id, Seq, Time};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// A decision without a timed grant has no finite clock deadline.
const NO_DEADLINE: i64 = i64::MAX;
/// Epochs begin at zero, so this value always asks the access engine again.
const UNDECIDED_EPOCH: i64 = -1;

/// What the access engine says about a person and a feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// They may follow it.
    Allowed,
    /// They may follow it as an admin who reads a chat they are not in: they
    /// are left out of its presence and cannot type.
    Inspecting,
    /// What it is about is there, and they hold nothing that lets them.
    Forbidden,
    /// There is nothing it could be about, or this server does not serve it.
    NotFound,
    /// What it was about was deleted.
    Gone,
}

/// What is kept for a subscription behind its lock.
#[derive(Debug)]
pub(crate) struct SubState {
    /// One actual thread projection, never a cache shared with another subscription.
    pub(crate) thread_projection: Option<orch_api_contract::threads::ThreadContinuationProjection>,
    /// An execution-scope change stops this subscription until a newly scoped HTTP cut/subscription.
    pub(crate) thread_reset: bool,
    /// Highest raw number admitted to this exact segment source, seeded at subscription.
    pub(crate) raw_after: i64,
    /// Commit-ordered usage changes already projected for this connection (never sent to it).
    pub(crate) usage_read: Option<i64>,
    /// Last delivered provider-health measures, excluding the moving window's timestamp.
    pub(crate) health_read: Option<orch_types::Hash>,
    /// Last native resource view delivered to this recipient, excluding database-only bookkeeping.
    pub(crate) resource_read: Option<orch_types::Hash>,
    /// The last session-personalized document metadata view; never a cross-reader rich cache.
    pub(crate) document_tree: Option<super::document_tree::State>,
    /// How far rows were read and sent.
    pub(crate) cursor: SendCursor,
    /// The highest number whose rows were read for it.
    pub(crate) read: Seq,
    /// The authorization epoch its access was last decided under.
    pub(crate) epoch: i64,
    /// What the epochs of who may do what came to then: only a move of theirs may have taken away a
    /// nested row or a private member that was sent; a thing made or a grant moves the one above alone.
    authority: i64,
    /// The session mode which produced this decision; it has no global epoch write.
    as_admin: bool,
    /// Reauthentication to another session also changes its proof/administration boundary.
    session: Id,
    /// The earliest timed authority that may change what this reader may see.
    valid_until: Option<Time>,
    /// No further durable data is sent until the former projection is reloaded.
    reload: bool,
    /// False once it ended: nothing more is sent on it.
    pub(crate) open: bool,
    /// Whether this reader already heard of the current outage.
    pub(crate) offline: bool,
}

/// One feed followed by one connection.
#[derive(Debug)]
pub struct Subscription {
    tag: Tag,
    feed: Feed,
    source: Option<Source>,
    connection: Arc<Connection>,
    sub_streams: bool,
    thread_options: ThreadOptions,
    live: AtomicBool,
    read: AtomicI64,
    decided: AtomicI64,
    decided_as_admin: AtomicBool,
    decided_session: Mutex<Id>,
    deadline: AtomicI64,
    clock: Arc<dyn Clock>,
    inner: Weak<Inner>,
    refreshing: AtomicBool,
    inspecting: AtomicBool,
    typing: Mutex<RateLimit>,
    pub(crate) state: tokio::sync::Mutex<SubState>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ThreadOptions {
    sub_streams: bool,
    raw_segment: Option<Id>,
    continuation: Option<Id>,
    revision: Option<i64>,
}

impl Subscription {
    fn new(
        tag: Tag,
        feed: Feed,
        source: Option<Source>,
        connection: Arc<Connection>,
        options: ThreadOptions,
        clock: Arc<dyn Clock>,
        inner: Weak<Inner>,
    ) -> Self {
        metrics::gauge!("orch_ws_subscriptions","feed_kind"=>metric_kind(&feed)).increment(1.0);
        let session = connection.session();
        Self {
            tag,
            feed,
            source,
            connection,
            sub_streams: options.sub_streams,
            thread_options: options,
            live: AtomicBool::new(false),
            read: AtomicI64::new(0),
            decided: AtomicI64::new(UNDECIDED_EPOCH),
            decided_as_admin: AtomicBool::new(false),
            decided_session: Mutex::new(session),
            deadline: AtomicI64::new(NO_DEADLINE),
            clock,
            inner,
            refreshing: AtomicBool::new(false),
            inspecting: AtomicBool::new(false),
            typing: Mutex::new(send_limit()),
            state: tokio::sync::Mutex::new(SubState {
                thread_projection: None,
                thread_reset: false,
                raw_after: 0,
                usage_read: None,
                health_read: None,
                resource_read: None,
                document_tree: None,
                cursor: SendCursor::starting_at(0),
                read: 0,
                epoch: 0,
                authority: 0,
                as_admin: false,
                session,
                valid_until: None,
                reload: false,
                open: true,
                offline: false,
            }),
        }
    }

    /// The number by which the client names it, and the one that tells it from
    /// any other the process has had.
    #[must_use]
    pub fn tag(&self) -> Tag {
        self.tag
    }

    /// The feed.
    #[must_use]
    pub fn feed(&self) -> &Feed {
        &self.feed
    }

    pub(crate) fn thread_scope(&self, state: &SubState) -> threads::ReadScope {
        threads::ReadScope {
            continuation: self.thread_options.continuation,
            revision: state
                .thread_projection
                .as_ref()
                .map(|projection| projection.projection_revision)
                .or(self.thread_options.revision),
        }
    }

    /// Native source is immutable even when several subscriptions share the same feed string.
    pub(crate) fn raw_segment(&self) -> Option<Id> {
        self.thread_options.raw_segment
    }

    pub(crate) fn source(&self) -> Option<Source> {
        self.source
    }

    /// The connection that follows it.
    #[must_use]
    pub fn connection(&self) -> &Arc<Connection> {
        &self.connection
    }

    /// Whether the client asked for the growing text of sub-agents' items.
    #[must_use]
    pub fn sub_streams(&self) -> bool {
        self.sub_streams
    }

    /// Whether it is set up and not ended: growing text is sent on it.
    #[must_use]
    pub fn is_live(&self) -> bool {
        if !self.live.load(Ordering::Acquire)
            || !self.connection.authenticated_at(self.clock.time())
        {
            return false;
        }
        if self.expired()
            || self.decided_as_admin.load(Ordering::Acquire) != self.connection.as_admin()
            || *self
                .decided_session
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                != self.connection.session()
        {
            self.refresh_after_expiry();
            return false;
        }
        true
    }

    fn expired(&self) -> bool {
        self.clock.time().unix_millis() >= self.deadline.load(Ordering::Acquire)
    }

    fn refresh_after_expiry(&self) {
        if self.refreshing.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let Some(sub) = self
            .connection
            .subscriptions()
            .into_iter()
            .find(|sub| sub.tag().generation == self.tag.generation)
        else {
            return;
        };
        // Presence and typing also use this cheap gate. They withhold content at the
        // deadline and schedule the same check that sends the subscription's closure.
        inner.tasks.clone().spawn(async move {
            let result = async {
                let epoch = crate::realtime::sync::read_epoch(&inner).await?;
                let mut st = sub.state.lock().await;
                inner.recheck(&sub, &mut st, epoch).await?;
                Ok::<(), Fail>(())
            }
            .await;
            sub.refreshing.store(false, Ordering::Release);
            if let Err(error) = result {
                tracing::warn!(%error, "an expired feed decision could not be refreshed");
            }
        });
    }

    pub(crate) fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Release);
    }

    /// The highest number whose rows were read for it, without waiting for
    /// the lock that guards the rest.
    pub(crate) fn read(&self) -> Seq {
        self.read.load(Ordering::Acquire)
    }

    pub(crate) fn set_read(&self, seq: Seq) {
        self.read.store(seq, Ordering::Release);
    }

    /// The authorization epoch its access was last decided under, without
    /// waiting for the lock that guards the rest.
    pub(crate) fn decided(&self) -> i64 {
        if self.expired() {
            UNDECIDED_EPOCH
        } else {
            self.decided.load(Ordering::Acquire)
        }
    }

    pub(crate) fn set_decided(&self, epoch: i64) {
        *self
            .decided_session
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = self.connection.session();
        self.decided_as_admin
            .store(self.connection.as_admin(), Ordering::Release);
        self.decided.store(epoch, Ordering::Release);
    }

    fn set_deadline(&self, until: Option<Time>) {
        self.deadline.store(
            until.map_or(NO_DEADLINE, Time::unix_millis),
            Ordering::Release,
        );
    }

    /// Whether its person reads the chat without being in it: an admin.
    #[must_use]
    pub fn inspecting(&self) -> bool {
        self.inspecting.load(Ordering::Acquire)
    }

    pub(crate) fn set_inspecting(&self, inspecting: bool) {
        self.inspecting.store(inspecting, Ordering::Release);
    }

    /// Whether a `typing` frame at `now` is let through: one in three seconds
    /// is, and the rest are dropped.
    pub(crate) fn admit_typing(&self, now: orch_types::Time) -> bool {
        self.typing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .admit(now)
    }
}

fn metric_kind(feed: &Feed) -> &'static str {
    match feed {
        Feed::Thread(_) | Feed::ThreadRaw(_) | Feed::ThreadBackground(_) | Feed::ThreadGit(_) => {
            "thread"
        }
        Feed::Topic(_) => "topic",
        Feed::Org(_) => "org",
        Feed::User => "user",
        Feed::Chat(_) => "chat",
        Feed::Doc(_) | Feed::DocumentTree(_) => "document",
        Feed::Env(_)
        | Feed::EnvStats(_)
        | Feed::EnvProcs(_)
        | Feed::EnvBrowsers(_)
        | Feed::EnvDisplays(_)
        | Feed::EnvDir { .. }
        | Feed::EnvTerminals(_)
        | Feed::EnvPorts(_)
        | Feed::EnvGit { .. } => "environment",
        Feed::Resource { .. } => "resource",
        Feed::Usage(_) => "usage",
        Feed::Job(_) | Feed::Fork(_) => "job",
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        metrics::gauge!("orch_ws_subscriptions","feed_kind"=>metric_kind(&self.feed))
            .decrement(1.0);
    }
}

// ---------------------------------------------------------------- the decision

/// The node-level answer of the access engine, as a feed needs it: whether
/// the person holds the View permission of the node.
pub(crate) async fn may_view(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
) -> Result<Access, Fail> {
    let view = PermissionName::OfNode(NodePermission::View);
    match nodes::check_named(conn, principal, node, view, Act::Use, Policy::default()).await {
        Ok(Decision::Allowed { .. }) => Ok(Access::Allowed),
        Ok(Decision::Refused(_)) => Ok(Access::Forbidden),
        Err(orch_authz::Error::NotFound(_)) => Ok(Access::NotFound),
        // Held back by a suspension or an archive: not theirs to follow now.
        Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::Archived { .. }) => {
            Ok(Access::Forbidden)
        }
        Err(error) => Err(error.into()),
    }
}

tokio::task_local! {
    /// The browser session a feed is decided for, where the decision is a connection's: the session a
    /// second step is given in.
    static VIEWING: Id;
}

/// Every projection uses this connection's current browser session, including
/// after reauthentication. A person's other session cannot supply its mode.
pub(crate) async fn viewing<T>(
    connection: &Connection,
    work: impl std::future::Future<Output = T>,
) -> T {
    VIEWING.scope(connection.session(), work).await
}

/// The person as the access engine knows them now, if there is such a person. Where the decision is a
/// connection's ([`decide_for`]), they are held out of the team orgs that wait for their second step as a
/// request of that session would be: while none is set up, or while this session has not given it.
pub(crate) async fn principal_of(
    inner: &Inner,
    conn: &mut Conn,
    user: Id,
) -> Result<Option<Principal>, Fail> {
    let viewing = VIEWING.try_with(|session| *session).ok();
    let as_admin = match viewing {
        Some(session) => {
            orch_db::queries::identity::session_admin_mode(conn, user, session, inner.clock.now())
                .await?
                .unwrap_or(false)
        }
        None => false,
    };
    orch_db::audit::set_admin_mode(user, as_admin);
    let principal = match Principal::load(
        conn,
        Caller::session(user).with_admin_mode(as_admin),
        inner.clock.time(),
    )
    .await
    {
        Ok(principal) => principal,
        Err(orch_authz::Error::NoSuchUser(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Some(session) = viewing else {
        return Ok(Some(principal));
    };
    let Some(person) = orch_db::queries::identity::user_by_id(conn, user).await? else {
        return Ok(None);
    };
    let viewer = crate::middleware::authentication::Authed {
        as_admin,
        user: person,
        via: crate::middleware::authentication::Via::Session {
            session_id: session,
        },
    };
    crate::sign_in::second_step::hold(conn, &viewer, principal)
        .await
        .map(Some)
        .map_err(Fail::other)
}

/// Whether the person of `connection` may follow `feed`, now, in the session the connection signed in
/// with: the decision of [`decide`], with the hold for a second step that session has not given.
pub(crate) async fn decide_for(
    inner: &Inner,
    connection: &Connection,
    feed: &Feed,
) -> Result<Access, Fail> {
    VIEWING
        .scope(connection.session(), decide(inner, connection.user, feed))
        .await
}

/// Loads a delivery's principal with this exact browser session's second-step holds.
pub(crate) async fn principal_for_connection(
    inner: &Inner,
    conn: &mut Conn,
    connection: &Connection,
) -> Result<Option<Principal>, Fail> {
    VIEWING
        .scope(
            connection.session(),
            principal_of(inner, conn, connection.user),
        )
        .await
}

/// Whether `user` may follow `feed`, now.
pub(crate) async fn decide(inner: &Inner, user: Id, feed: &Feed) -> Result<Access, Fail> {
    match feed {
        Feed::Job(_) | Feed::Fork(_) => {
            super::machine_views::decide_progress(inner, user, feed).await
        }
        Feed::Usage(scope) => super::usage::decide(inner, user, scope).await,
        Feed::Thread(id) => threads::decide(inner, user, *id).await,
        Feed::ThreadBackground(id) => threads::decide(inner, user, *id).await,
        Feed::ThreadGit(_) => super::machine_views::decide_git(inner, user, feed).await,
        Feed::Topic(id) => topics::decide(inner, user, *id).await,
        Feed::Org(id) => tree::decide(inner, user, *id).await,
        Feed::User => Ok(Access::Allowed),
        Feed::Chat(id) => chats::decide(inner, user, *id).await,
        Feed::Doc(id) => documents::decide(inner, user, *id).await,
        Feed::DocumentTree(view) => super::document_tree::decide(inner, user, view).await,
        // What an environment's processes, load and directories need is on the environment's page.
        Feed::EnvProcs(_)
        | Feed::EnvStats(_)
        | Feed::EnvTerminals(_)
        | Feed::EnvPorts(_)
        | Feed::EnvGit { .. }
        | Feed::EnvDir { .. }
        | Feed::EnvBrowsers(_)
        | Feed::EnvDisplays(_) => match watches::needs(feed) {
            Some((environment, permission)) => {
                watches::decide(inner, user, environment, permission).await
            }
            None => Ok(Access::NotFound),
        },
        Feed::Resource { .. } => super::resources::decide(inner, user, feed).await,
        Feed::ThreadRaw(thread) => super::resources::raw_access(inner, user, *thread).await,
        // An environment's own feed is read by whoever may read the environment.
        Feed::Env(id) => super::environments::decide(inner, user, *id).await,
    }
}

// ---------------------------------------------------------------- registering

impl Inner {
    /// Adds a subscription to the feeds the server reads for.
    pub(crate) fn register(&self, sub: &Arc<Subscription>) {
        let Some(source) = sub.source() else {
            self.ephemeral.register(sub);
            return;
        };
        let mut registry = self.registry();
        let entry = registry
            .sources
            .entry(source)
            .or_insert_with(|| crate::realtime::Entry {
                subs: Vec::new(),
                notified: 0,
                running: false,
                dirty: false,
                last: None,
                streaming: None,
                waiting: Vec::new(),
                carrying: false,
            });
        entry.subs.push(Arc::clone(sub));
    }

    /// Takes a subscription out of the feeds the server reads for.
    pub(crate) fn unregister(&self, sub: &Arc<Subscription>) {
        let Some(source) = sub.source() else {
            self.ephemeral.unregister(sub);
            return;
        };
        let mut registry = self.registry();
        let Some(entry) = registry.sources.get_mut(&source) else {
            return;
        };
        entry.subs.retain(|each| !Arc::ptr_eq(each, sub));
        // A task that is reading for the feed removes it when it finds nobody.
        if entry.subs.is_empty() && !entry.running {
            registry.sources.remove(&source);
        }
    }

    /// Ends a subscription in the server's books and tells its client why.
    pub(crate) async fn close_sub(
        self: &Arc<Self>,
        sub: &Arc<Subscription>,
        st: &mut SubState,
        reason: FeedClosedReason,
    ) {
        if !st.open {
            return;
        }
        st.open = false;
        sub.set_live(false);
        self.unregister(sub);
        let connection = sub.connection();
        connection.forget(sub);
        // Rows that were read for it and have not left yet are not sent: the
        // decision they were read under no longer holds.
        connection.outbox.purge(sub.tag().generation);
        if let Ok(text) = (ServerFrame::Closed(ClosedFrame {
            id: sub.tag().id,
            feed: sub.feed().clone(),
            reason,
        }))
        .encode()
        {
            connection.outbox.push_frame(sub.tag(), text);
        }
        self.leave_presence(connection, sub.feed());
    }

    /// Revalidates a cached decision before delivery when its epoch or deadline ended.
    pub(crate) async fn recheck(
        self: &Arc<Self>,
        sub: &Arc<Subscription>,
        st: &mut SubState,
        epoch: i64,
    ) -> Result<bool, Fail> {
        self.recheck_delivery(sub, st, epoch, false).await
    }

    /// Inbox rows may need a new HTTP cut while this still-open listener receives
    /// freshly projected personal news. Neither lane can revive an ended subscription.
    pub(crate) async fn recheck_personal(
        self: &Arc<Self>,
        sub: &Arc<Subscription>,
        st: &mut SubState,
        epoch: i64,
    ) -> Result<bool, Fail> {
        if !matches!(sub.feed(), Feed::User) {
            return Ok(false);
        }
        self.recheck_delivery(sub, st, epoch, true).await
    }

    async fn recheck_delivery(
        self: &Arc<Self>,
        sub: &Arc<Subscription>,
        st: &mut SubState,
        epoch: i64,
        personal: bool,
    ) -> Result<bool, Fail> {
        let paused_inbox = st.reload && matches!(sub.feed(), Feed::User);
        if !st.open
            || (st.reload && !matches!(sub.feed(), Feed::User))
            || st.thread_reset
            || sub.connection().outbox.closing().is_some()
        {
            return Ok(false);
        }
        if !watches::credential_current(self, sub.connection()).await? {
            return Ok(false);
        }
        if st.epoch == epoch
            && st.as_admin == sub.connection().as_admin()
            && st.session == sub.connection().session()
            && st.valid_until.is_none_or(|until| self.clock.time() < until)
        {
            return Ok(!st.reload || personal);
        }
        // Read the deadline before deciding. If it passes while deciding, this decision
        // remains expired instead of being cached as though that grant had no end.
        let until = viewing(
            sub.connection(),
            watches::earliest_end(self, sub.connection().user),
        )
        .await?;
        // Read before deciding: a change of authority that commits after it moves the epoch
        // again, and is asked about then.
        let authority = {
            let mut conn = self.db.acquire().await?;
            orch_db::queries::access::authority_epoch(&mut conn).await?
        };
        // What may have been taken away from what was sent: a change of who may do what, or of
        // the session's mode or proof, or an announcement of this person's identity (which sets the
        // epoch to -1 and rechecks every socket of theirs conservatively: a second step or a
        // credential of theirs may have changed). Something made, or only given, takes nothing away.
        let narrowed = st
            .valid_until
            .is_some_and(|until| self.clock.time() >= until)
            || st.epoch < 0
            || st.authority != authority
            || st.as_admin != sub.connection().as_admin()
            || st.session != sub.connection().session();
        match decide_for(self, sub.connection(), sub.feed()).await? {
            access @ (Access::Allowed | Access::Inspecting) => {
                if matches!(sub.feed(), Feed::DocumentTree(_)) {
                    st.document_tree = None;
                    if sub.live.load(Ordering::Acquire) {
                        self.ephemeral.document_tree_changed(None);
                    }
                }
                st.epoch = epoch;
                st.authority = authority;
                st.as_admin = sub.connection().as_admin();
                st.session = sub.connection().session();
                st.valid_until = until;
                sub.set_deadline(until);
                sub.set_decided(epoch);
                sub.set_inspecting(access == Access::Inspecting);
                if until.is_some_and(|until| self.clock.time() >= until) {
                    return Ok(false);
                }
                if narrowed && (sub.live.load(Ordering::Acquire) || paused_inbox) {
                    // An authority change may narrow a nested row or private member while
                    // leaving the feed readable. Drop its old projection before more data.
                    let purged = sub
                        .connection()
                        .outbox
                        .purge_projection(sub.tag().generation);
                    if purged.resource {
                        // This view was only queued, never handed to the socket. Its
                        // replacement must not be mistaken for an already delivered row.
                        st.resource_read = None;
                    }
                    // A Resource event replaces its complete recipient view. Its native
                    // projection/fingerprint, rather than an epoch alone, decides delivery.
                    if !matches!(sub.feed(), Feed::Resource { .. }) && (!st.reload || purged.resync)
                    {
                        let text = ServerFrame::Resync(ResyncFrame {
                            id: sub.tag().id,
                            feed: sub.feed().clone(),
                            reload: Some(true),
                        })
                        .encode()
                        .map_err(Fail::other)?;
                        sub.connection().outbox.push_resync(sub.tag(), text);
                    }
                    if sub.source().is_some() || matches!(sub.feed(), Feed::ThreadRaw(_)) {
                        st.reload = true;
                        sub.set_live(false);
                        return Ok(personal);
                    }
                }
                Ok(!st.reload || personal)
            }
            Access::Forbidden => {
                self.close_sub(sub, st, FeedClosedReason::Revoked).await;
                Ok(false)
            }
            Access::NotFound | Access::Gone => {
                self.close_sub(sub, st, FeedClosedReason::Deleted).await;
                Ok(false)
            }
        }
    }

    /// Asks again for every subscription of a connection that was last decided
    /// under an older epoch.
    pub(crate) async fn recheck_connection(
        self: &Arc<Self>,
        connection: &Arc<Connection>,
        epoch: i64,
    ) -> Result<(), Fail> {
        for sub in connection.subscriptions() {
            let mut st = sub.state.lock().await;
            self.recheck(&sub, &mut st, epoch).await?;
        }
        Ok(())
    }

    /// What the presence frame of `feed` says now.
    pub(crate) async fn presence_frame(
        &self,
        tag: Tag,
        feed: &Feed,
    ) -> Result<Option<String>, Fail> {
        let users = self.presence_users(feed).await?;
        Ok(ServerFrame::Presence(PresenceFrame {
            id: tag.id,
            feed: feed.clone(),
            users,
        })
        .encode()
        .ok())
    }
}

// ---------------------------------------------------------------- what a client asks

fn refuse(connection: &Connection, id: SubId, code: SocketErrorCode, detail: &str) {
    connection.error(Some(id), code, detail);
}

/// A `sub` frame: the decision, the answer, and the rows that were missed.
///
/// A mistake of the client's (a feed that is no feed's, one it may not
/// follow, one too many) is answered with an `error` frame and the connection
/// stays open. A subscription whose id is that of one that is open takes its
/// place: the old one ends without a word, since a `closed` for the id would
/// be taken by the client for the end of the new one.
pub(crate) async fn subscribe(
    inner: &Arc<Inner>,
    connection: &Arc<Connection>,
    frame: SubFrame,
) -> Result<(), Fail> {
    let id = frame.id;
    let feed = match frame.feed() {
        Ok(feed) => feed,
        Err(bad) => {
            refuse(connection, id, SocketErrorCode::BadFeed, &bad.to_string());
            return Ok(());
        }
    };
    let raw = matches!(feed, Feed::ThreadRaw(_));
    if raw
        && (frame.raw_segment_id.is_none()
            || frame
                .projection_revision
                .is_none_or(|revision| revision < 1))
    {
        refuse(
            connection,
            id,
            SocketErrorCode::BadFeed,
            "A raw subscription needs raw_segment_id and a positive projection_revision.",
        );
        return Ok(());
    }
    if !raw && frame.raw_segment_id.is_some() {
        refuse(
            connection,
            id,
            SocketErrorCode::BadFeed,
            "raw_segment_id applies only to raw protocol traffic.",
        );
        return Ok(());
    }
    if !matches!(feed, Feed::Thread(_) | Feed::ThreadRaw(_))
        && (frame.continuation_id.is_some() || frame.projection_revision.is_some())
    {
        refuse(
            connection,
            id,
            SocketErrorCode::BadFeed,
            "A continuation selector applies only to a thread's conversation or raw feed.",
        );
        return Ok(());
    }
    let mut require_full = false;
    if let Some(old) = connection.take(id) {
        let mut st = old.state.lock().await;
        require_full = !raw && old.feed() == &feed && st.reload && frame.since.is_some();
        st.open = false;
        old.set_live(false);
        inner.unregister(&old);
        // The same feed under the same number goes on being looked at.
        if old.feed() != &feed {
            inner.leave_presence(connection, old.feed());
        }
    }
    if !subscription_allowed(connection.held()) {
        refuse(
            connection,
            id,
            SocketErrorCode::TooManySubscriptions,
            &format!("a connection follows at most {MAX_SUBSCRIPTIONS} feeds"),
        );
        return Ok(());
    }
    let source = Source::of(&feed, connection.user);
    if source.is_none() && feed.kind() != orch_realtime_contract::feeds::FeedKind::Ephemeral {
        refuse(
            connection,
            id,
            SocketErrorCode::NotFound,
            "this server does not carry that feed",
        );
        return Ok(());
    };
    // The epochs are read before the decision: if they move meanwhile, the
    // decision is asked again with the next one.
    let epoch = crate::realtime::sync::read_epoch(inner).await?;
    let authority = {
        let mut conn = inner.db.acquire().await?;
        orch_db::queries::access::authority_epoch(&mut conn).await?
    };
    let until = viewing(connection, watches::earliest_end(inner, connection.user)).await?;
    let access = decide_for(inner, connection, &feed).await?;
    match access {
        Access::Allowed | Access::Inspecting => {}
        Access::Forbidden => {
            refuse(
                connection,
                id,
                SocketErrorCode::Forbidden,
                "you may not follow that feed",
            );
            return Ok(());
        }
        Access::NotFound | Access::Gone => {
            refuse(
                connection,
                id,
                SocketErrorCode::NotFound,
                "there is nothing by that name",
            );
            return Ok(());
        }
    }
    if require_full {
        // An incremental cursor cannot clear private fields retained under an older grant.
        let text = ServerFrame::Subscribed(SubscribedFrame {
            raw: None,
            projection: None,
            id,
            feed,
            seq: None,
            mode: SubscribeMode::Reload,
        })
        .encode()
        .map_err(Fail::other)?;
        connection.outbox.push_frame(
            Tag {
                id,
                generation: inner.number(),
            },
            text,
        );
        return Ok(());
    }
    let tag = Tag {
        id,
        generation: inner.number(),
    };
    let sub = Arc::new(Subscription::new(
        tag,
        feed.clone(),
        source,
        Arc::clone(connection),
        ThreadOptions {
            sub_streams: frame.sub_streams.unwrap_or(false),
            raw_segment: frame.raw_segment_id,
            continuation: frame.continuation_id,
            revision: frame.projection_revision,
        },
        Arc::clone(&inner.clock),
        Arc::downgrade(inner),
    ));
    sub.set_inspecting(access == Access::Inspecting);
    inner_opened(
        inner,
        connection,
        &feed,
        frame.opening_id.unwrap_or_else(orch_types::new_id),
    )
    .await?;
    // Nothing is done for it until it is set up: whoever wants the lock waits.
    let mut st = sub.state.lock().await;
    st.epoch = epoch;
    st.authority = authority;
    st.as_admin = connection.as_admin();
    st.session = connection.session();
    st.valid_until = until;
    sub.set_deadline(until);
    sub.set_decided(epoch);
    connection.add(&sub);
    inner.register(&sub);
    let outcome = viewing(
        connection,
        begin(inner, connection, &sub, &mut st, frame.since),
    )
    .await;
    match outcome {
        Ok(true) => {
            sub.set_live(true);
            inner.watch_job(connection, &feed);
            drop(st);
            // Whatever was announced while it was being set up is read now.
            if let Some(source) = source {
                inner.poke(source);
            }
            Ok(())
        }
        Ok(false) => {
            st.open = false;
            inner.unregister(&sub);
            connection.forget(&sub);
            Ok(())
        }
        Err(error) => {
            st.open = false;
            inner.unregister(&sub);
            connection.forget(&sub);
            Err(error)
        }
    }
}

async fn inner_opened(
    inner: &Arc<Inner>,
    connection: &Connection,
    feed: &Feed,
    opening: Id,
) -> Result<(), Fail> {
    let (kind, id) = match feed {
        Feed::Thread(id)
        | Feed::ThreadRaw(id)
        | Feed::ThreadBackground(id)
        | Feed::ThreadGit(id) => ("thread", *id),
        Feed::Topic(id) => ("topic", *id),
        Feed::Org(id) => ("org", *id),
        Feed::Doc(id) => ("document", *id),
        Feed::Chat(id) => ("chat", *id),
        Feed::DocumentTree(
            orch_realtime_contract::document_tree::DocumentTreeView::Directory {
                at: orch_api_contract::document_tree::DocumentDirectory::Container(id),
                deleted: None,
                ..
            },
        ) => ("document_container", *id),
        Feed::DocumentTree(orch_realtime_contract::document_tree::DocumentTreeView::Entry {
            item: orch_api_contract::document_tree::DocumentTreeItem::Document(id),
            inspect_deleted: false,
        }) => ("document", *id),
        Feed::DocumentTree(orch_realtime_contract::document_tree::DocumentTreeView::Entry {
            item: orch_api_contract::document_tree::DocumentTreeItem::Container(id),
            inspect_deleted: false,
        }) => ("document_container", *id),
        Feed::Env(id)
        | Feed::EnvStats(id)
        | Feed::EnvProcs(id)
        | Feed::EnvBrowsers(id)
        | Feed::EnvDisplays(id)
        | Feed::EnvTerminals(id)
        | Feed::EnvPorts(id)
        | Feed::EnvDir {
            environment_id: id, ..
        }
        | Feed::EnvGit {
            environment_id: id, ..
        } => ("environment", *id),
        Feed::Resource { kind, id } => (kind.as_str(), *id),
        Feed::User | Feed::Usage(_) | Feed::Job(_) | Feed::Fork(_) | Feed::DocumentTree(_) => {
            return Ok(());
        }
    };
    let mut tx = inner.db.begin().await?;
    let Some(principal) = principal_for_connection(inner, &mut tx, connection).await? else {
        return Ok(());
    };
    let place = crate::middleware::admin_openings::private_place(&mut tx, &principal, kind, id)
        .await
        .map_err(Fail::other)?;
    let mut origin = connection.origin.clone();
    origin.as_admin = principal.caller().as_admin().then_some(connection.user);
    if let Some(place) = place {
        let node = match place {
            orch_access_contract::roles::PlaceRef::Node(id) => Some(id),
            _ => None,
        };
        orch_db::audit::scope_origin(
            origin.clone(),
            orch_db::audit::opened(
                &mut tx,
                orch_db::audit::Opening {
                    user: connection.user,
                    session: connection.session(),
                    nonce: opening,
                    kind,
                    object: id,
                    place: crate::middleware::admin_openings::stored_place(place),
                    node,
                },
                &origin,
                inner.clock.now(),
            ),
        )
        .await?;
    } else if let Feed::Chat(chat) = feed {
        // Explicit Directs View can still inspect a people's chat without native mode.
        orch_db::audit::scope_origin(
            origin.clone(),
            orch_db::audit::scope_browser_opening(
                Some(connection.session()),
                opening,
                viewing(
                    connection,
                    chats::opened(inner, &mut tx, connection.user, *chat, &origin),
                ),
            ),
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Answers a subscription and sends it the rows it missed. False when the
/// answer was to load again, in which case the subscription is not kept.
async fn begin(
    inner: &Arc<Inner>,
    connection: &Arc<Connection>,
    sub: &Arc<Subscription>,
    st: &mut SubState,
    since: Option<Seq>,
) -> Result<bool, Fail> {
    let feed = sub.feed();
    let latest = crate::realtime::sync::read_epoch(inner).await?;
    if !inner.recheck(sub, st, latest).await? {
        return Ok(false);
    }
    if feed.kind() == orch_realtime_contract::feeds::FeedKind::Ephemeral {
        let Some(_identity) = watches::delivery_guard(inner, connection).await? else {
            return Ok(false);
        };
        if matches!(feed, Feed::ThreadRaw(_)) {
            return super::raw_traffic::begin(inner, sub, st).await;
        }
        let text = ServerFrame::Subscribed(SubscribedFrame {
            raw: None,
            projection: None,
            id: sub.tag().id,
            feed: feed.clone(),
            seq: None,
            mode: SubscribeMode::Ephemeral,
        })
        .encode()
        .map_err(Fail::other)?;
        connection.outbox.push_frame(sub.tag(), text);
        st.offline = inner.ephemeral.current(inner, sub, st).await?;
        return Ok(st.open);
    }
    let (head, scope_current) = if let Feed::Thread(thread) = feed {
        match threads::head_scoped(inner, *thread, since, sub.thread_scope(st)).await? {
            Some((head, projection, current)) => {
                st.thread_projection = Some(projection);
                (Some(head), current)
            }
            None => (None, true),
        }
    } else {
        (
            crate::realtime::sync::head(inner, feed, connection.user, since).await?,
            true,
        )
    };
    let Some(Head {
        feed: head,
        rows_after,
    }) = head
    else {
        // What it was about went between the decision and now.
        connection.error(
            Some(sub.tag().id),
            SocketErrorCode::NotFound,
            "there is nothing by that name",
        );
        return Ok(false);
    };
    let mut answer = head.answer(feed.kind(), since, rows_after);
    if !scope_current {
        answer.mode = SubscribeMode::Reload;
        answer.seq = Some(head.seq);
    }
    let text = ServerFrame::Subscribed(SubscribedFrame {
        raw: None,
        projection: st.thread_projection.clone(),
        id: sub.tag().id,
        feed: feed.clone(),
        seq: answer.seq,
        mode: answer.mode,
    })
    .encode()
    .map_err(Fail::other)?;
    connection.outbox.push_frame(sub.tag(), text);
    if answer.mode == SubscribeMode::Reload {
        return Ok(false);
    }
    let start = answer.seq.unwrap_or(head.seq);
    st.cursor = SendCursor::starting_at(start);
    st.read = start;
    sub.set_read(start);
    if feed.has_presence() {
        // Who is looking, even when nobody is: a client that connects again
        // drops the list it had.
        if let Some(text) = inner.presence_frame(sub.tag(), feed).await? {
            connection.outbox.push_frame(sub.tag(), text);
        }
    }
    inner
        .deliver(sub, st, head.seq, &mut Cache::default())
        .await?;
    Ok(true)
}

/// An `unsub` frame. Every one is answered once: with `closed` and the reason
/// `unsubscribed` when the subscription was open, and with an `error` that
/// says there is no such subscription when it was not.
pub(crate) async fn unsubscribe(
    inner: &Arc<Inner>,
    connection: &Arc<Connection>,
    frame: UnsubFrame,
) {
    let Some(sub) = connection.take(frame.id) else {
        connection.error(
            Some(frame.id),
            SocketErrorCode::NotFound,
            "that subscription is not open",
        );
        return;
    };
    let mut st = sub.state.lock().await;
    inner
        .close_sub(&sub, &mut st, FeedClosedReason::Unsubscribed)
        .await;
}
