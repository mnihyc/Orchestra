//! Resources on the small world of the tests: providers, hosts, environments and the rest, with their pages, and the calls that ask about them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::pages::{self, ChangeError, Outcome, PageView, Target};
use orch_authz::resources::{self, Seen};
use orch_authz::usable::{
    self, Author, Policies, ResourcePolicy, TurnAsk, TurnDecision, Usable, Usables,
};
use orch_authz::{Caller, Policy, Principal};
use orch_db::queries::access::{NewEntry, PageKey, insert_entry, testing::add_entry};
use orch_db::queries::resources::access_fixtures;
use orch_types::{Id, new_id};
use serde_json::json;

use super::{World, stored};

/// The object a resource of `kind` and `id` is.
pub fn object(kind: &str, id: Id) -> ObjectRef {
    ObjectRef {
        kind: kind.parse::<ObjectKind>().unwrap(),
        id,
    }
}

impl World {
    /// A resource of `kind` at home `home` (an org; for an environment its home node), first owned
    /// by `owner`, who is named in its Owners row.
    pub async fn thing(&self, kind: &str, owner: Id, home: Id, name: &str) -> Id {
        let mut conn = self.conn().await;
        let id = access_fixtures::insert_resource(&mut conn, kind, Some(owner), Some(home), name)
            .await
            .unwrap();
        add_entry(
            &mut conn,
            &PageKey::new(kind, id, "live"),
            "any",
            "access.own",
            ("user", Some(owner), None),
            None,
            self.now.get(),
        )
        .await
        .unwrap();
        id
    }

    /// A resource of the system: a home of none, and an owner who counts for nothing.
    pub async fn system_thing(&self, kind: &str, name: &str) -> Id {
        let mut conn = self.conn().await;
        access_fixtures::insert_resource(&mut conn, kind, None, None, name)
            .await
            .unwrap()
    }

    /// An API provider at home `home`, first owned by `owner`.
    pub async fn provider(&self, owner: Id, home: Id, name: &str) -> Id {
        self.thing("provider", owner, home, name).await
    }

    /// A model of a provider that lists some efforts.
    pub async fn model(&self, provider: Id, name: &str, efforts: &[&str]) {
        let mut conn = self.conn().await;
        access_fixtures::insert_model(&mut conn, provider, name, efforts, true, self.boss)
            .await
            .unwrap();
    }

    /// Names `holder` in the row `permission` of the page of the resource `id`, with options.
    pub async fn put(
        &self,
        kind: &str,
        id: Id,
        permission: Permission,
        holder: Holder,
        options: serde_json::Value,
    ) -> Id {
        let mut conn = self.conn().await;
        let entry = new_id();
        let (holder_kind, holder_id, holder_role) = stored(&holder);
        insert_entry(
            &mut conn,
            &NewEntry {
                id: entry,
                page: &PageKey::new(kind, id, "live"),
                profile: "any",
                permission: permission.as_str(),
                holder_kind,
                holder_id,
                holder_role,
                options: &options,
                expires_at: None,
                note: "",
                source: "page",
                created_by: None,
                at: self.now.get(),
            },
        )
        .await
        .unwrap();
        entry
    }

    /// Names `holder` in the row `permission` of a resource's page, with no options.
    pub async fn grant(&self, kind: &str, id: Id, permission: Permission, holder: Holder) -> Id {
        self.put(kind, id, permission, holder, json!({})).await
    }

    /// Stores the row of a resource's own page as it is given.
    pub async fn thing_row(&self, kind: &str, id: Id, permission: Permission, inherit: bool) {
        let mut conn = self.conn().await;
        orch_db::queries::access::testing::set_row(
            &mut conn,
            &PageKey::new(kind, id, "live"),
            "any",
            permission.as_str(),
            inherit,
            false,
        )
        .await
        .unwrap();
    }

    /// Whether the user, in a session, holds `permission` on the resource.
    pub async fn can_on(&self, user: Id, kind: &str, id: Id, permission: Permission) -> bool {
        self.decide_on(Caller::session(user), kind, id, permission)
            .await
            .is_allowed()
    }

    /// The decision about a resource for the caller.
    pub async fn decide_on(
        &self,
        caller: Caller,
        kind: &str,
        id: Id,
        permission: Permission,
    ) -> Decision {
        let principal = self.principal(caller).await;
        let mut conn = self.conn().await;
        resources::check(
            &mut conn,
            &principal,
            object(kind, id),
            permission,
            Policy::default(),
        )
        .await
        .unwrap()
    }

    /// Everything the user holds on a resource, as permissions.
    pub async fn holds_on(&self, user: Id, kind: &str, id: Id) -> Vec<Permission> {
        let principal = self.principal(Caller::session(user)).await;
        let mut conn = self.conn().await;
        resources::effective(&mut conn, &principal, object(kind, id), Policy::default())
            .await
            .unwrap()
            .held
            .into_iter()
            .map(|held| held.permission)
            .collect()
    }

    /// Whether the caller may use the resource for the work at `node`.
    pub async fn usable(&self, caller: Caller, id: Id, node: Option<Id>) -> Usable {
        self.usable_under(caller, id, node, &Policies::default())
            .await
    }

    pub async fn usable_under(
        &self,
        caller: Caller,
        id: Id,
        node: Option<Id>,
        policies: &Policies,
    ) -> Usable {
        let principal = self.principal(caller).await;
        let mut conn = self.conn().await;
        usable::usable(&mut conn, &principal, id, node, policies)
            .await
            .unwrap()
    }

    /// A turn of the thread on the provider, for the author, with a model and an effort.
    pub async fn turn(
        &self,
        author: Id,
        thread: Id,
        provider: Id,
        model: &str,
        effort: Option<&str>,
    ) -> TurnDecision {
        self.turn_as(
            Caller::session(author),
            thread,
            provider,
            model,
            effort,
            &Policies::default(),
        )
        .await
    }

    pub async fn turn_as(
        &self,
        caller: Caller,
        thread: Id,
        provider: Id,
        model: &str,
        effort: Option<&str>,
        policies: &Policies,
    ) -> TurnDecision {
        let principal = self.principal(caller).await;
        let mut conn = self.conn().await;
        usable::check_turn(
            &mut conn,
            Author::Person(&principal),
            &TurnAsk {
                thread,
                provider,
                profile: None,
                model,
                effort,
                arrangement: None,
                models_allowed: None,
            },
            policies,
        )
        .await
        .unwrap()
    }

    /// The resources of a kind that the caller may use for the work at `node`.
    pub async fn usable_list(&self, user: Id, kind: &str, node: Option<Id>) -> Vec<Usables> {
        let principal = self.principal(Caller::session(user)).await;
        let mut conn = self.conn().await;
        usable::usable_list(
            &mut conn,
            &principal,
            kind.parse().unwrap(),
            node,
            &Policies::default(),
            orch_authz::nodes::Page {
                after: None,
                limit: 1000,
            },
        )
        .await
        .unwrap()
        .items
    }

    /// The resources of a kind that the user holds anything on.
    pub async fn visible(&self, user: Id, kind: &str) -> Vec<Seen> {
        let principal = self.principal(Caller::session(user)).await;
        let mut conn = self.conn().await;
        resources::visible(
            &mut conn,
            &principal,
            kind.parse().unwrap(),
            orch_authz::nodes::Page {
                after: None,
                limit: 1000,
            },
            Policy::default(),
        )
        .await
        .unwrap()
        .items
    }

    pub async fn principal_of(&self, user: Id) -> Principal {
        self.principal(Caller::session(user)).await
    }

    /// Explicitly selects session administration for this one resource-page act.
    pub async fn change_thing_as_admin(
        &self,
        user: Id,
        kind: &str,
        id: Id,
        change: serde_json::Value,
    ) -> Result<Outcome, ChangeError> {
        self.change_thing_as(
            Caller::session(user).with_admin_mode(true),
            kind,
            id,
            change,
            &ResourcePolicy::default(),
        )
        .await
    }

    /// Changes the page of a resource as the user, in a transaction that is kept when the change is
    /// made and dropped when it is refused.
    pub async fn change_thing(
        &self,
        user: Id,
        kind: &str,
        id: Id,
        change: serde_json::Value,
    ) -> Result<Outcome, ChangeError> {
        self.change_thing_as(
            Caller::session(user),
            kind,
            id,
            change,
            &ResourcePolicy::default(),
        )
        .await
    }

    pub async fn change_thing_as(
        &self,
        caller: Caller,
        kind: &str,
        id: Id,
        change: serde_json::Value,
        policy: &ResourcePolicy,
    ) -> Result<Outcome, ChangeError> {
        let change: orch_access_contract::pages::PageChange =
            serde_json::from_value(change).expect("a change that reads");
        let principal = self.principal(caller).await;
        let mut tx = self.db.pool.begin().await.unwrap();
        let outcome = pages::change_resource(
            &mut tx,
            &principal,
            object(kind, id),
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

    /// A resource's page, read as the user.
    pub async fn read_thing(&self, user: Id, kind: &str, id: Id) -> PageView {
        self.read_thing_as(Caller::session(user), kind, id).await
    }

    /// Reads a resource page with the explicitly supplied caller context.
    pub async fn read_thing_as(&self, caller: Caller, kind: &str, id: Id) -> PageView {
        let principal = self.principal(caller).await;
        let mut conn = self.conn().await;
        pages::read(
            &mut conn,
            &principal,
            Target::Resource(object(kind, id)),
            Policy::default(),
        )
        .await
        .unwrap()
    }

    /// Puts the thread into the environment.
    pub async fn put_in(&self, thread: Id, environment: Id) {
        let mut conn = self.conn().await;
        access_fixtures::put_thread_into(&mut conn, thread, environment)
            .await
            .unwrap();
    }

    /// Binds the thread to the provider it runs on.
    pub async fn bind_thread(&self, thread: Id, provider: Id) {
        let mut conn = self.conn().await;
        sqlx::query("UPDATE threads SET provider_id = $2 WHERE node_id = $1")
            .bind(thread)
            .bind(provider)
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    /// A thread of the topic first owned by its maker, running in the environment.
    pub async fn thread_in(&self, topic: Id, environment: Id, name: &str) -> Id {
        let thread = self.node("thread", topic, name).await;
        self.put_in(thread, environment).await;
        thread
    }
}
