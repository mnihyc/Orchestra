//! A small world on a real database for the tests of the access engine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

pub mod things;

use chrono::Duration;
use orch_access_contract::decisions::Decision;
use orch_access_contract::pages::{self as contract_pages, Holder};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_access_contract::roles::{NodeKind, Role};
use orch_authz::engine::{Frame, Graph};
use orch_authz::pages::{self, ChangeError, Outcome, PageView, Target};
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::access::{PageKey, SYSTEM_ID, testing::*};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Time, new_id};

/// Words for a holder as an entry keeps it.
pub fn stored(holder: &Holder) -> (&'static str, Option<Id>, Option<&'static str>) {
    let (kind, id, role) = holder.stored();
    (kind.as_str(), id, role.map(Role::as_str))
}

/// A tree of one root org with a project, a topic and a thread, with the
/// system's defaults as shipped. A `boss` owns the org, the project and the
/// topic, and is so an Admin of them; the `maker` owns the thread alone. Both
/// are members of the root org.
pub struct World {
    pub db: TestDb,
    pub now: Time,
    pub boss: Id,
    pub maker: Id,
    pub root: Id,
    pub project: Id,
    pub topic: Id,
    pub thread: Id,
}

impl World {
    pub async fn new() -> Self {
        let db = TestDb::fresh().await;
        let now = Time::new(clock::start());
        let mut conn = db.pool.acquire().await.unwrap();
        let (boss, maker) = (new_id(), new_id());
        insert_user(&mut conn, boss, "boss").await.unwrap();
        insert_user(&mut conn, maker, "maker").await.unwrap();
        let (root, project, topic, thread) = (new_id(), new_id(), new_id(), new_id());
        insert_node(&mut conn, root, "org", None, "Acme", boss)
            .await
            .unwrap();
        insert_member(&mut conn, root, boss, root).await.unwrap();
        insert_member(&mut conn, root, maker, root).await.unwrap();
        insert_node(&mut conn, project, "project", Some(root), "Web", boss)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Login", boss)
            .await
            .unwrap();
        insert_node(&mut conn, thread, "thread", Some(topic), "Fix", maker)
            .await
            .unwrap();
        for node in [root, project, topic] {
            name_owner(&mut conn, node, ("user", boss), now.get())
                .await
                .unwrap();
        }
        name_owner(&mut conn, thread, ("user", maker), now.get())
            .await
            .unwrap();
        seed_shipped_defaults(&mut conn, now).await;
        drop(conn);
        Self {
            db,
            now,
            boss,
            maker,
            root,
            project,
            topic,
            thread,
        }
    }

    pub async fn conn(&self) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
        self.db.pool.acquire().await.unwrap()
    }

    /// A user with a name, who belongs to no org.
    pub async fn user(&self, name: &str) -> Id {
        let mut conn = self.conn().await;
        let id = new_id();
        insert_user(&mut conn, id, name).await.unwrap();
        id
    }

    /// A user who is a member of the root org, with the root as their domain.
    pub async fn member(&self, name: &str) -> Id {
        let id = self.user(name).await;
        let mut conn = self.conn().await;
        insert_member(&mut conn, self.root, id, self.root)
            .await
            .unwrap();
        id
    }

    /// A node of `kind` inside `parent`, first owned by the boss, or by the
    /// maker for a thread (who is named in its Owners row).
    pub async fn node(&self, kind: &str, parent: Id, name: &str) -> Id {
        let owner = if kind == "thread" {
            self.maker
        } else {
            self.boss
        };
        self.node_owned_by(kind, parent, name, owner).await
    }

    pub async fn node_owned_by(&self, kind: &str, parent: Id, name: &str, owner: Id) -> Id {
        let mut conn = self.conn().await;
        let id = new_id();
        insert_node(&mut conn, id, kind, Some(parent), name, owner)
            .await
            .unwrap();
        name_owner(&mut conn, id, ("user", owner), self.now.get())
            .await
            .unwrap();
        id
    }

    /// Gives `role` on `place` to the person `user`.
    pub async fn give(&self, place: Id, user: Id, role: Role) -> Id {
        let mut conn = self.conn().await;
        give_role(
            &mut conn,
            place,
            "user",
            user,
            role.as_str(),
            self.boss,
            None,
        )
        .await
        .unwrap()
    }

    /// Names `holder` in the row `permission` of the page of `node`.
    pub async fn name(&self, node: Id, permission: Permission, holder: Holder) -> Id {
        let mut conn = self.conn().await;
        add_entry(
            &mut conn,
            &PageKey::new("node", node, "live"),
            "any",
            permission.as_str(),
            stored(&holder),
            None,
            self.now.get(),
        )
        .await
        .unwrap()
    }

    /// Names `holder` in the defaults row `permission` of the place `place`
    /// (the system when none), for the profile `profile`.
    pub async fn name_default(
        &self,
        place: Option<Id>,
        permission: Permission,
        profile: &str,
        holder: Holder,
    ) -> Id {
        let mut conn = self.conn().await;
        let page = match place {
            Some(place) => PageKey::new("node", place, "defaults"),
            None => PageKey::new("system", SYSTEM_ID, "defaults"),
        };
        add_entry(
            &mut conn,
            &page,
            profile,
            permission.as_str(),
            stored(&holder),
            None,
            self.now.get(),
        )
        .await
        .unwrap()
    }

    /// Stores the row as it is given on the own page of `node`.
    pub async fn row(&self, node: Id, permission: Permission, inherit: bool) {
        let mut conn = self.conn().await;
        set_row(
            &mut conn,
            &PageKey::new("node", node, "live"),
            "any",
            permission.as_str(),
            inherit,
            false,
        )
        .await
        .unwrap();
    }

    /// Stores the row as it is given on the defaults page of `place`.
    pub async fn default_row(
        &self,
        place: Id,
        permission: Permission,
        profile: &str,
        inherit: bool,
        locked: bool,
    ) {
        let mut conn = self.conn().await;
        set_row(
            &mut conn,
            &PageKey::new("node", place, "defaults"),
            profile,
            permission.as_str(),
            inherit,
            locked,
        )
        .await
        .unwrap();
    }

    pub async fn principal(&self, caller: Caller) -> Principal {
        let mut conn = self.conn().await;
        Principal::load(&mut conn, caller, self.now).await.unwrap()
    }

    /// A tree of departments inside the root org, with a project, a topic and a
    /// thread three departments down, and some siblings.
    pub async fn deep(&self) -> Deep {
        let d1 = self.node("org", self.root, "Research").await;
        let d2 = self.node("org", d1, "ML").await;
        let d3 = self.node("org", d2, "Vision").await;
        let sales = self.node("org", self.root, "Sales").await;
        let ops = self.node("org", d1, "Ops").await;
        let project = self.node("project", d3, "Detect").await;
        let topic = self.node("topic", project, "Boxes").await;
        let other_topic = self.node("topic", project, "Masks").await;
        let thread = self.node("thread", topic, "Train").await;
        let sales_project = self.node("project", sales, "Deals").await;
        Deep {
            d1,
            d2,
            d3,
            sales,
            ops,
            project,
            topic,
            other_topic,
            thread,
            sales_project,
        }
    }

    /// A group kept by the org `org`, with these people in it.
    pub async fn group(&self, org: Id, name: &str, people: &[Id]) -> Id {
        let mut conn = self.conn().await;
        let group = insert_group(&mut conn, org, name, self.boss).await.unwrap();
        for person in people {
            add_to_group(&mut conn, group, *person).await.unwrap();
        }
        group
    }

    /// Whether the user, in a session, holds `permission` on `node` at `time`.
    pub async fn can_at(&self, user: Id, node: Id, permission: Permission, time: Time) -> bool {
        let mut conn = self.conn().await;
        let principal = Principal::load(&mut conn, Caller::session(user), time)
            .await
            .unwrap();
        nodes::check(
            &mut conn,
            &principal,
            node,
            permission,
            Act::Use,
            Policy::default(),
        )
        .await
        .unwrap()
        .is_allowed()
    }

    /// Whether the user, in a session, holds `permission` on `node` now.
    pub async fn can(&self, user: Id, node: Id, permission: Permission) -> bool {
        self.can_as(Caller::session(user), node, permission).await
    }

    pub async fn can_as(&self, caller: Caller, node: Id, permission: Permission) -> bool {
        self.decide(caller, node, permission, Act::Use)
            .await
            .is_allowed()
    }

    pub async fn decide(
        &self,
        caller: Caller,
        node: Id,
        permission: Permission,
        act: Act,
    ) -> Decision {
        let principal = self.principal(caller).await;
        let mut conn = self.conn().await;
        nodes::check(
            &mut conn,
            &principal,
            node,
            permission,
            act,
            Policy::default(),
        )
        .await
        .unwrap()
    }

    /// Everything the user holds on `node`, as permissions.
    pub async fn holds(&self, user: Id, node: Id) -> Vec<Permission> {
        self.holds_as(Caller::session(user), node).await
    }

    /// Everything the explicitly selected session or token holds on `node`.
    pub async fn holds_as(&self, caller: Caller, node: Id) -> Vec<Permission> {
        let principal = self.principal(caller).await;
        let mut conn = self.conn().await;
        nodes::effective(&mut conn, &principal, node, Policy::default())
            .await
            .unwrap()
            .held
            .into_iter()
            .map(|held| held.permission)
            .collect()
    }

    pub fn later(&self, by: Duration) -> Time {
        Time::new(self.now.get() + by)
    }

    /// Changes a page as the user, in a transaction that is kept when the change
    /// is made and dropped when it is refused.
    pub async fn change(
        &self,
        user: Id,
        target: Target,
        change: serde_json::Value,
    ) -> Result<Outcome, ChangeError> {
        self.change_as(Caller::session(user), target, change, Policy::default())
            .await
    }

    /// Selects session administration for this one page change.
    pub async fn change_as_admin(
        &self,
        user: Id,
        target: Target,
        change: serde_json::Value,
    ) -> Result<Outcome, ChangeError> {
        self.change_as(
            Caller::session(user).with_admin_mode(true),
            target,
            change,
            Policy::default(),
        )
        .await
    }

    pub async fn change_as(
        &self,
        caller: Caller,
        target: Target,
        change: serde_json::Value,
        policy: Policy,
    ) -> Result<Outcome, ChangeError> {
        self.change_at(caller, target, change, policy, self.now)
            .await
    }

    /// Changes a page as the caller at the moment `at`.
    pub async fn change_at(
        &self,
        caller: Caller,
        target: Target,
        change: serde_json::Value,
        policy: Policy,
        at: Time,
    ) -> Result<Outcome, ChangeError> {
        let change: orch_access_contract::pages::PageChange =
            serde_json::from_value(change).expect("a change that reads");
        let principal = {
            let mut conn = self.conn().await;
            Principal::load(&mut conn, caller, at).await.unwrap()
        };
        let mut tx = self.db.pool.begin().await.unwrap();
        let outcome = pages::change(
            &mut tx,
            &principal,
            target,
            &change,
            policy,
            &orch_db::audit::Origin::default(),
        )
        .await;
        if outcome.is_ok() {
            tx.commit().await.unwrap();
        }
        outcome
    }

    /// Changes the defaults page of a place as the user.
    pub async fn change_defaults(
        &self,
        user: Id,
        place: Id,
        change: serde_json::Value,
    ) -> Result<Outcome, ChangeError> {
        self.change(
            user,
            Target::Defaults(orch_access_contract::roles::PlaceRef::Node(place)),
            change,
        )
        .await
    }

    pub async fn read(&self, user: Id, target: Target) -> PageView {
        self.read_as(Caller::session(user), target).await
    }

    /// Reads with exactly the selected session or token context.
    pub async fn read_as(&self, caller: Caller, target: Target) -> PageView {
        let principal = self.principal(caller).await;
        let mut conn = self.conn().await;
        pages::read(&mut conn, &principal, target, Policy::default())
            .await
            .unwrap()
    }

    /// The audit rows so far, oldest first.
    pub async fn audit(&self) -> Vec<AuditRow> {
        sqlx::query_as(
            "SELECT action, history, override, node_id, object_kind, object_id, summary, details
               FROM audit_log ORDER BY at, id",
        )
        .fetch_all(&self.db.pool)
        .await
        .unwrap()
    }

    pub async fn epoch(&self) -> i64 {
        let mut conn = self.conn().await;
        orch_authz::epoch::current(&mut conn).await.unwrap()
    }
}

/// Departments inside the root org with a project, a topic and a thread in the
/// deepest, and siblings beside.
pub struct Deep {
    pub d1: Id,
    pub d2: Id,
    pub d3: Id,
    pub sales: Id,
    pub ops: Id,
    pub project: Id,
    pub topic: Id,
    pub other_topic: Id,
    pub thread: Id,
    pub sales_project: Id,
}

/// One row of the audit trail.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuditRow {
    pub action: String,
    pub history: bool,
    #[sqlx(rename = "override")]
    pub r#override: bool,
    pub node_id: Option<Id>,
    pub object_kind: String,
    pub object_id: Option<Id>,
    pub summary: String,
    pub details: serde_json::Value,
}

/// A holder as a request writes it.
pub fn user_holder(id: Id) -> serde_json::Value {
    serde_json::json!({ "kind": "user", "id": id })
}

pub fn role_holder(role: Role) -> serde_json::Value {
    serde_json::json!({ "kind": "role", "role": role.as_str() })
}

pub fn group_holder(id: Id) -> serde_json::Value {
    serde_json::json!({ "kind": "group", "id": id })
}

/// The system's defaults page as it ships.
pub async fn seed_shipped_defaults(conn: &mut Conn, now: Time) {
    let page = PageKey::new("system", SYSTEM_ID, "defaults");
    for shipped in contract_pages::shipped() {
        add_entry(
            conn,
            &page,
            shipped.profile.as_str(),
            shipped.permission.as_str(),
            ("role", None, Some(shipped.role.as_str())),
            None,
            now.get(),
        )
        .await
        .unwrap();
    }
}

/// Every permission there is on a node of `kind`.
pub fn permissions_of(kind: NodeKind) -> Vec<Permission> {
    ObjectKind::from(kind).permissions().collect()
}

/// The frame for deciding about `nodes` for `principal`, with every
/// permission read.
pub async fn frame_for<'p>(
    conn: &mut Conn,
    principal: &'p Principal,
    nodes: &[Id],
) -> (Frame<'p>, Vec<orch_authz::Site>) {
    let graph = Graph::load(conn, nodes).await.unwrap();
    let sites: Vec<_> = nodes.iter().filter_map(|id| graph.site_of(*id)).collect();
    let everything: Vec<Permission> = Permission::ALL.to_vec();
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        &sites,
        &everything,
    )
    .await
    .unwrap();
    (frame, sites)
}

/// Counts the statements that run on this thread: sqlx tells of each one as an
/// event. One subscriber is installed for the whole test binary, and it counts
/// per thread, so that tests that run side by side count their own.
pub struct Statements;

thread_local! {
    static COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Statements {
    /// Makes sure the subscriber is installed, and returns the count so far on
    /// this thread.
    pub fn now() -> usize {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = tracing::subscriber::set_global_default(Statements);
        });
        COUNT.with(std::cell::Cell::get)
    }
}

impl tracing::Subscriber for Statements {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            COUNT.with(|count| count.set(count.get() + 1));
        }
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}
