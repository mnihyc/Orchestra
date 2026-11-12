//! Properties that hold of every world of resources: taking an entry away or cutting a row loose never grows anybody's holding or use, a lock gives the same holders on every page below it, and a token never has what its user lacks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use std::collections::BTreeSet;

use chrono::Duration;
use orch_access_contract::decisions::{ObjectRef, Scopes};
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_access_contract::roles::Role;
use orch_authz::engine::Frame;
use orch_authz::resources;
use orch_authz::usable::{self, Policies, Usable};
use orch_authz::{Caller, Policy, TokenGrant};
use orch_db::queries::access::testing::{
    add_entry, give_role, insert_member, make_system_admin, set_row,
};
use orch_db::queries::access::{PageKey, remove_entry};
use orch_db::queries::resources::access_fixtures;
use orch_types::Id;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use serde_json::json;
use support::World;

/// The worlds each property is tried in: enough that a rule broken in one corner shows, few enough
/// to run in seconds.
const WORLDS: u64 = 5;

/// The models of every provider of a world, which what an entry limits is tried against.
const MODELS: [&str; 3] = ["m1", "m2", "m3"];

/// What a person holds on a resource: (person, resource, permission).
type Held = BTreeSet<(Id, Id, Permission)>;
/// What a person may run for a thread: (person, resource, thread, model); the model is empty for
/// a resource that takes no models.
type Uses = BTreeSet<(Id, Id, Option<Id>, &'static str)>;

struct Scene {
    w: World,
    rng: StdRng,
    users: Vec<Id>,
    places: Vec<Id>,
    groups: Vec<Id>,
    things: Vec<(ResourceKind, Id)>,
    threads: Vec<Id>,
}

fn pick<T: Copy>(rng: &mut StdRng, from: &[T]) -> T {
    from[rng.random_range(0..from.len())]
}

fn kind_word(kind: ResourceKind) -> &'static str {
    ObjectKind::from(kind).as_str()
}

impl Scene {
    async fn new(seed: u64) -> Self {
        let w = World::new().await;
        let mut rng = StdRng::seed_from_u64(seed);
        let d1 = w.node("org", w.root, "D1").await;
        let d2 = w.node("org", d1, "D2").await;
        let orgs = [w.root, d1, d2];
        let other_project = w.node("project", pick(&mut rng, &orgs), "Other").await;
        let other_topic = w.node("topic", other_project, "Elsewhere").await;
        let away = w.node("thread", other_topic, "Away").await;
        let mut users = Vec::new();
        for index in 0..4 {
            let user = w.user(&format!("user{index}")).await;
            if rng.random_bool(0.7) {
                let domain = pick(&mut rng, &orgs);
                let mut conn = w.conn().await;
                insert_member(&mut conn, w.root, user, domain)
                    .await
                    .unwrap();
            }
            users.push(user);
        }
        let admin = w.user("sysadmin").await;
        {
            let mut conn = w.conn().await;
            make_system_admin(&mut conn, admin).await.unwrap();
        }
        users.push(admin);
        let mut groups = Vec::new();
        for index in 0..2 {
            let members: Vec<Id> = users
                .iter()
                .copied()
                .filter(|_| rng.random_bool(0.4))
                .collect();
            groups.push(
                w.group(pick(&mut rng, &orgs), &format!("G{index}"), &members)
                    .await,
            );
        }
        let mut places: Vec<Id> = orgs.to_vec();
        places.extend([w.project, w.topic, other_project, other_topic]);
        let mut things: Vec<(ResourceKind, Id)> = Vec::new();
        let kinds = [
            (ResourceKind::Provider, 2),
            (ResourceKind::Host, 1),
            (ResourceKind::Template, 2),
            (ResourceKind::Mount, 1),
            (ResourceKind::Agent, 1),
            (ResourceKind::McpServer, 1),
        ];
        for (kind, count) in kinds {
            for index in 0..count {
                let home = pick(&mut rng, &orgs);
                let owner = pick(&mut rng, &users);
                let name = format!("{}{index}", kind_word(kind).replace('_', "-"));
                let id = w.thing(kind_word(kind), owner, home, &name).await;
                if kind == ResourceKind::Provider {
                    for model in MODELS {
                        w.model(id, model, &["low", "high"]).await;
                    }
                }
                things.push((kind, id));
            }
        }
        let environment = w
            .thing(
                "environment",
                pick(&mut rng, &users),
                pick(&mut rng, &[w.topic, w.project]),
                "env",
            )
            .await;
        things.push((ResourceKind::Environment, environment));
        let mut conn = w.conn().await;
        let system =
            access_fixtures::insert_resource(&mut conn, "template", None, None, "system-template")
                .await
                .unwrap();
        drop(conn);
        things.push((ResourceKind::Template, system));
        let mut scene = Self {
            w,
            rng,
            users,
            places,
            groups,
            things,
            threads: vec![],
        };
        scene.threads = vec![scene.w.thread, away];
        scene.populate().await;
        scene
    }

    fn holder(&mut self, system: bool) -> (&'static str, Option<Id>, Option<&'static str>) {
        let role = pick(&mut self.rng, Role::ALL).as_str();
        let pool = if system { 6 } else { 5 };
        match self.rng.random_range(0..pool) {
            0 => ("role", None, Some(role)),
            1 => ("role", Some(pick(&mut self.rng, &self.places)), Some(role)),
            2 => ("user", Some(pick(&mut self.rng, &self.users)), None),
            3 => ("group", Some(pick(&mut self.rng, &self.groups)), None),
            4 => ("node", Some(pick(&mut self.rng, &self.places)), None),
            _ => ("everyone", None, None),
        }
    }

    async fn populate(&mut self) {
        let now = self.w.now.get();
        for _ in 0..10 {
            let place = pick(&mut self.rng, &self.places[..5]);
            let role = pick(&mut self.rng, Role::ALL).as_str();
            let (kind, subject) = match self.rng.random_range(0..3) {
                0 => ("user", pick(&mut self.rng, &self.users)),
                1 => ("group", pick(&mut self.rng, &self.groups)),
                _ => ("members", pick(&mut self.rng, &self.places[..3])),
            };
            let mut conn = self.w.conn().await;
            let _ = give_role(&mut conn, place, kind, subject, role, self.w.boss, None).await;
        }
        for _ in 0..40 {
            let (kind, id) = pick(&mut self.rng, &self.things.clone());
            let system = self.things.iter().position(|(_, other)| *other == id)
                == Some(self.things.len() - 1);
            let object = ObjectKind::from(kind);
            let permissions: Vec<Permission> = object
                .permissions()
                .filter(|permission| *permission != Permission::AccessOwn)
                .collect();
            let permission = pick(&mut self.rng, &permissions);
            let (holder_kind, holder_id, holder_role) = self.holder(system);
            // Only the Use row takes the work in a place; the system's own resource may name everyone.
            if holder_kind == "node"
                && permission.level() != orch_access_contract::permissions::Level::Use
            {
                continue;
            }
            if holder_kind == "everyone" && !system {
                continue;
            }
            let page = PageKey::new(kind_word(kind), id, "live");
            let options = if permission == Permission::ProviderUse && self.rng.random_bool(0.6) {
                let mut models: Vec<&str> = MODELS.to_vec();
                models.retain(|_| self.rng.random_bool(0.5));
                json!({ "models": models })
            } else {
                json!({})
            };
            let expires = match self.rng.random_range(0..7) {
                0 => Some(now - Duration::hours(1)),
                1 => Some(now + Duration::days(1)),
                _ => None,
            };
            let cut = self.rng.random_bool(0.3);
            let mut conn = self.w.conn().await;
            if cut {
                set_row(&mut conn, &page, "any", permission.as_str(), false, false)
                    .await
                    .unwrap();
            }
            if let Ok(entry) = add_entry(
                &mut conn,
                &page,
                "any",
                permission.as_str(),
                (holder_kind, holder_id, holder_role),
                expires,
                now,
            )
            .await
            {
                sqlx::query("UPDATE permission_entries SET options = $2 WHERE id = $1")
                    .bind(entry)
                    .bind(options)
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                if self.rng.random_bool(0.12) {
                    remove_entry(&mut conn, entry, self.w.boss, now)
                        .await
                        .unwrap();
                }
            }
        }
        // Defaults of the orgs, for the rows of resources.
        for _ in 0..10 {
            let place = pick(&mut self.rng, &self.places[..3]);
            let (kind, _) = pick(&mut self.rng, &self.things.clone());
            if kind == ResourceKind::Environment {
                continue;
            }
            let permissions: Vec<Permission> = ObjectKind::from(kind)
                .permissions()
                .filter(|permission| *permission != Permission::AccessOwn)
                .collect();
            let permission = pick(&mut self.rng, &permissions);
            let (holder_kind, holder_id, holder_role) = self.holder(false);
            if holder_kind == "everyone" || holder_kind == "node" {
                continue;
            }
            let mut conn = self.w.conn().await;
            let _ = add_entry(
                &mut conn,
                &PageKey::new("node", place, "defaults"),
                "any",
                permission.as_str(),
                (holder_kind, holder_id, holder_role),
                None,
                now,
            )
            .await;
        }
    }

    /// Everything each person holds on each resource, for callers made by `caller`.
    async fn held(&self, caller: impl Fn(Id) -> Caller) -> Held {
        let mut held = Held::new();
        for user in &self.users {
            let principal = self.w.principal(caller(*user)).await;
            for (kind, id) in &self.things {
                let mut conn = self.w.conn().await;
                let Ok(effective) = resources::effective(
                    &mut conn,
                    &principal,
                    ObjectRef {
                        kind: ObjectKind::from(*kind),
                        id: *id,
                    },
                    Policy::default(),
                )
                .await
                else {
                    continue;
                };
                for each in effective.held {
                    held.insert((*user, *id, each.permission));
                }
            }
        }
        held
    }

    /// What each person may run, for each resource and each thread.
    async fn uses(&self, caller: impl Fn(Id) -> Caller) -> Uses {
        let mut uses = Uses::new();
        let nodes = [None, Some(self.threads[0]), Some(self.threads[1])];
        for user in &self.users {
            let principal = self.w.principal(caller(*user)).await;
            for (kind, id) in &self.things {
                if !kind.has_use() {
                    continue;
                }
                for node in nodes {
                    let mut conn = self.w.conn().await;
                    let found =
                        usable::usable(&mut conn, &principal, *id, node, &Policies::default())
                            .await;
                    let Ok(Usable::Yes(granted)) = found else {
                        continue;
                    };
                    if *kind == ResourceKind::Provider {
                        for model in MODELS {
                            if granted.named_together().allows_model(model) {
                                uses.insert((*user, *id, node, model));
                            }
                        }
                    } else {
                        uses.insert((*user, *id, node, ""));
                    }
                }
            }
        }
        uses
    }

    async fn some_entry(&mut self) -> Option<Id> {
        let entries: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM permission_entries WHERE removed_at IS NULL AND permission <> 'access.own' AND object_kind <> 'node' ORDER BY id",
        )
        .fetch_all(&self.w.db.pool)
        .await
        .unwrap();
        (!entries.is_empty()).then(|| pick(&mut self.rng, &entries))
    }
}

#[tokio::test]
async fn nobodys_holding_or_use_of_a_resource_grows_when_an_entry_is_removed_or_expires() {
    let mut took_something_away = 0_usize;
    for seed in 0..WORLDS {
        let mut scene = Scene::new(seed + 300).await;
        let held_before = scene.held(Caller::session).await;
        let uses_before = scene.uses(Caller::session).await;
        assert!(
            !held_before.is_empty() && !uses_before.is_empty(),
            "seed {seed}: a world where nobody holds or uses anything tests nothing"
        );
        for round in 0..2 {
            let Some(entry) = scene.some_entry().await else {
                break;
            };
            let now = scene.w.now.get();
            if round % 2 == 0 {
                let mut conn = scene.w.conn().await;
                remove_entry(&mut conn, entry, scene.w.boss, now)
                    .await
                    .unwrap();
            } else {
                sqlx::query("UPDATE permission_entries SET expires_at = $2 WHERE id = $1")
                    .bind(entry)
                    .bind(now - Duration::seconds(1))
                    .execute(&scene.w.db.pool)
                    .await
                    .unwrap();
            }
            let held_after = scene.held(Caller::session).await;
            let uses_after = scene.uses(Caller::session).await;
            let held_gain: Vec<_> = held_after.difference(&held_before).collect();
            let uses_gain: Vec<_> = uses_after.difference(&uses_before).collect();
            assert!(
                held_gain.is_empty(),
                "seed {seed}: removing an entry gave {held_gain:?}"
            );
            assert!(
                uses_gain.is_empty(),
                "seed {seed}: removing an entry gave the use of {uses_gain:?}"
            );
            took_something_away += usize::from(
                held_after.len() < held_before.len() || uses_after.len() < uses_before.len(),
            );
        }
    }
    assert!(
        took_something_away > 0,
        "the worlds are such that removing an entry sometimes takes something away"
    );
}

#[tokio::test]
async fn cutting_a_row_of_a_resource_loose_never_grows_anyones_holding_or_use() {
    let mut took_something_away = 0_usize;
    for seed in 0..WORLDS {
        let mut scene = Scene::new(seed + 400).await;
        let held_before = scene.held(Caller::session).await;
        let uses_before = scene.uses(Caller::session).await;
        for _ in 0..2 {
            let (kind, id) = pick(&mut scene.rng, &scene.things.clone());
            let permissions: Vec<Permission> = ObjectKind::from(kind)
                .permissions()
                .filter(|permission| *permission != Permission::AccessOwn)
                .collect();
            let permission = pick(&mut scene.rng, &permissions);
            let mut conn = scene.w.conn().await;
            set_row(
                &mut conn,
                &PageKey::new(kind_word(kind), id, "live"),
                "any",
                permission.as_str(),
                false,
                false,
            )
            .await
            .unwrap();
            drop(conn);
            let held_after = scene.held(Caller::session).await;
            let uses_after = scene.uses(Caller::session).await;
            assert!(
                held_after.difference(&held_before).next().is_none(),
                "seed {seed}: cutting {permission} loose gave something"
            );
            assert!(
                uses_after.difference(&uses_before).next().is_none(),
                "seed {seed}: cutting {permission} loose gave a use"
            );
            took_something_away += usize::from(
                held_after.len() < held_before.len() || uses_after.len() < uses_before.len(),
            );
        }
    }
    assert!(
        took_something_away > 0,
        "the worlds are such that cutting a row loose sometimes takes something away"
    );
}

#[tokio::test]
async fn a_token_never_has_what_its_user_lacks_on_a_resource() {
    let mut narrower = 0_usize;
    for seed in 0..WORLDS {
        let mut scene = Scene::new(seed + 500).await;
        let session_held = scene.held(Caller::session).await;
        let session_uses = scene.uses(Caller::session).await;
        assert!(!session_held.is_empty());
        let subtree = pick(&mut scene.rng, &scene.places.clone());
        let scopes: Vec<Permission> = Permission::ALL
            .iter()
            .copied()
            .filter(|_| scene.rng.random_bool(0.5))
            .collect();
        let grants = [
            TokenGrant::everything(),
            TokenGrant::new(Scopes::Only(scopes.iter().copied().collect()), None),
            TokenGrant::new(
                Scopes::Only(scopes.iter().copied().collect()),
                Some(subtree),
            ),
        ];
        for grant in grants {
            let with = |user: Id| Caller::token(user, grant.clone());
            let held = scene.held(with).await;
            let uses = scene.uses(with).await;
            assert!(
                held.difference(&session_held).next().is_none(),
                "seed {seed}: a token held {:?} that its user lacks",
                held.difference(&session_held).collect::<Vec<_>>()
            );
            assert!(
                uses.difference(&session_uses).next().is_none(),
                "seed {seed}: a token used {:?} that its user lacks",
                uses.difference(&session_uses).collect::<Vec<_>>()
            );
            narrower +=
                usize::from(held.len() < session_held.len() || uses.len() < session_uses.len());
        }
    }
    assert!(narrower > 0, "a token sometimes has less");
}

#[tokio::test]
async fn a_locked_row_gives_the_same_holders_to_every_page_of_a_resource_below_it() {
    for seed in 0..WORLDS {
        let scene = Scene::new(seed + 600).await;
        let root = scene.w.root;
        // The root org locks the Use row of a provider and of a template, naming Developers.
        for permission in [Permission::ProviderUse, Permission::TemplateUse] {
            // An entry like this may be there already, from the world's own defaults.
            let mut conn = scene.w.conn().await;
            let _ = add_entry(
                &mut conn,
                &PageKey::new("node", root, "defaults"),
                "any",
                permission.as_str(),
                ("role", None, Some(Role::Developer.as_str())),
                None,
                scene.w.now.get(),
            )
            .await;
            drop(conn);
            scene
                .w
                .default_row(root, permission, "any", true, true)
                .await;
        }
        let principal = scene.w.principal(Caller::session(scene.users[0])).await;
        for permission in [Permission::ProviderUse, Permission::TemplateUse] {
            let mut holders: Vec<Vec<Id>> = Vec::new();
            for (kind, id) in &scene.things {
                let wanted = if permission == Permission::ProviderUse {
                    ResourceKind::Provider
                } else {
                    ResourceKind::Template
                };
                if *kind != wanted {
                    continue;
                }
                let mut conn = scene.w.conn().await;
                let (graph, resource, site) = resources::site(
                    &mut conn,
                    ObjectRef {
                        kind: ObjectKind::from(*kind),
                        id: *id,
                    },
                )
                .await
                .unwrap();
                if resource.of_system() {
                    continue;
                }
                let frame = Frame::load(
                    &mut conn,
                    &principal,
                    Policy::default(),
                    graph,
                    std::slice::from_ref(&site),
                    &[permission],
                )
                .await
                .unwrap();
                let mut entries: Vec<Id> = frame
                    .chain(&site, permission)
                    .holders(frame.now())
                    .into_iter()
                    .map(|named| named.entry.id)
                    .collect();
                entries.sort();
                holders.push(entries);
            }
            assert!(
                holders.len() >= 2,
                "seed {seed}: at least two pages below the lock"
            );
            assert!(
                holders.windows(2).all(|pair| pair[0] == pair[1]),
                "seed {seed}: {permission} gives different holders below a lock: {holders:?}"
            );
            assert!(!holders[0].is_empty());
        }
    }
}

async fn compare_selected_participation(
    w: &World,
    caller: Caller,
    provider: Id,
) -> (bool, bool, bool, bool) {
    let principal = w.principal(caller).await;
    let mut conn = w.conn().await;
    let object = ObjectRef {
        kind: ObjectKind::Provider,
        id: provider,
    };
    let (graph, _, site) = resources::site(&mut conn, object).await.unwrap();
    let sites = [site.clone()];
    let permissions: Vec<_> = site.kind.permissions().collect();
    let full = Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph.clone(),
        &sites,
        &permissions,
    )
    .await
    .unwrap();
    let selected = Frame::load_with_participation(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        &sites,
        &permissions,
        &sites,
    )
    .await
    .unwrap();
    assert_eq!(selected.effective(&site), full.effective(&site));
    assert_eq!(selected.participates(&site), full.participates(&site));
    assert_eq!(selected.admin_only(&site), full.admin_only(&site));
    assert_eq!(selected.admin_access(&site), full.admin_access(&site));
    assert_eq!(selected.admin_place(&site), full.admin_place(&site));
    let shown = resources::effective(&mut conn, &principal, object, Policy::default())
        .await
        .unwrap();
    assert_eq!(shown.held, full.effective(&site));
    assert_eq!(shown.participating, full.participates(&site));
    assert_eq!(shown.admin_only, full.admin_only(&site));
    assert_eq!(shown.admin_access, full.admin_access(&site));
    let home = full.graph().site_of(w.root).unwrap();
    let homes = [home.clone()];
    let home_permissions: Vec<_> = home.kind.permissions().collect();
    let home_full = Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        full.graph().clone(),
        &homes,
        &home_permissions,
    )
    .await
    .unwrap();
    let home_selected = Frame::load_with_participation(
        &mut conn,
        &principal,
        Policy::default(),
        full.graph().clone(),
        &homes,
        &home_permissions,
        &homes,
    )
    .await
    .unwrap();
    let home_without_metadata = Frame::load_with_participation(
        &mut conn,
        &principal,
        Policy::default(),
        full.graph().clone(),
        &homes,
        &home_permissions,
        &[],
    )
    .await
    .unwrap();
    // A selected node still needs descendant/follow summaries: it must use the full SQL path.
    assert_eq!(
        home_selected.participates(&home),
        home_full.participates(&home)
    );
    for frame in [&home_selected, &home_without_metadata] {
        assert_eq!(frame.effective(&home), home_full.effective(&home));
        assert_eq!(frame.holds_any(&home), home_full.holds_any(&home));
        assert_eq!(frame.admin_only(&home), home_full.admin_only(&home));
        assert_eq!(frame.admin_access(&home), home_full.admin_access(&home));
        assert_eq!(frame.admin_place(&home), home_full.admin_place(&home));
    }
    assert!(!home_without_metadata.participates(&home));
    if principal.caller().as_admin() {
        assert!(
            !full.admin_only(&site).is_empty(),
            "the native baseline is exercised"
        );
    }
    let known = home_full.holds_any(&home) || principal.is_in_tree(home.root.unwrap());
    (
        full.participates(&site),
        full.effective(&site)
            .iter()
            .any(|held| held.permission == Permission::ProviderView),
        known,
        home_full.participates(&home),
    )
}

#[tokio::test]
async fn selected_resource_metadata_keeps_permissions_names_and_admin_baselines() {
    let w = World::new().await;
    let provider = {
        let mut conn = w.conn().await;
        let provider = access_fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(w.boss),
            Some(w.root),
            "Metadata provider",
        )
        .await
        .unwrap();
        add_entry(
            &mut conn,
            &PageKey::new("provider", provider, "live"),
            "any",
            "access.own",
            ("user", Some(w.boss), None),
            None,
            w.now.get(),
        )
        .await
        .unwrap();
        provider
    };
    let owner = compare_selected_participation(&w, Caller::session(w.boss), provider).await;
    assert!(owner.0 && owner.1 && owner.2);
    let member = compare_selected_participation(&w, Caller::session(w.maker), provider).await;
    assert!(
        !member.0 && member.2,
        "membership names the org without creating personal resource participation"
    );
    let reader = w.user("groupreader").await;
    let group = w.group(w.root, "Readers", &[reader]).await;
    let grant = {
        let mut conn = w.conn().await;
        add_entry(
            &mut conn,
            &PageKey::new("provider", provider, "live"),
            "any",
            "provider.view",
            ("group", Some(group), None),
            Some(w.now.get() + Duration::days(1)),
            w.now.get(),
        )
        .await
        .unwrap()
    };
    let shared = compare_selected_participation(&w, Caller::session(reader), provider).await;
    assert!(
        shared.0 && shared.1 && !shared.2 && shared.3,
        "resource participation grants no hidden home name"
    );
    for subtree in [w.root, w.project] {
        compare_selected_participation(
            &w,
            Caller::token(reader, TokenGrant::new(Scopes::All, Some(subtree))),
            provider,
        )
        .await;
    }
    {
        let mut conn = w.conn().await;
        sqlx::query("UPDATE users SET status='suspended' WHERE id=$1")
            .bind(reader)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let suspended = compare_selected_participation(&w, Caller::session(reader), provider).await;
    assert!(!suspended.0 && !suspended.1);
    {
        let mut conn = w.conn().await;
        sqlx::query("UPDATE users SET status='active' WHERE id=$1")
            .bind(reader)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE permission_entries SET expires_at=$2 WHERE id=$1")
            .bind(grant)
            .bind(w.now.get())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let expired = compare_selected_participation(&w, Caller::session(reader), provider).await;
    assert!(!expired.0 && !expired.1);
    let administrator = w.user("metaadmin").await;
    {
        let mut conn = w.conn().await;
        make_system_admin(&mut conn, administrator).await.unwrap();
    }
    let native = compare_selected_participation(
        &w,
        Caller::session(administrator).with_admin_mode(true),
        provider,
    )
    .await;
    assert!(
        !native.0 && native.1 && native.2,
        "native reach does not manufacture ordinary participation"
    );
}

async fn compare_batched_participation(w: &World, caller: Caller, ids: &[Id]) -> Vec<bool> {
    let principal = w.principal(caller).await;
    let mut conn = w.conn().await;
    let mut graph = orch_authz::engine::Graph::default();
    let found = resources::read_into(&mut conn, &mut graph, ids)
        .await
        .unwrap();
    let sites: Vec<_> = found
        .iter()
        .map(|resource| resources::site_of(&graph, resource).unwrap())
        .collect();
    assert_eq!(sites.len(), ids.len());
    let permissions: Vec<_> = ObjectKind::Provider.permissions().collect();
    let full = Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        &sites,
        &permissions,
    )
    .await
    .unwrap();
    let among = resources::effective_among(&mut conn, &principal, ids, Policy::default())
        .await
        .unwrap();
    assert_eq!(among.len(), ids.len());
    for shown in &among {
        let site = sites
            .iter()
            .find(|site| site.id == shown.object.id)
            .unwrap();
        let active = !principal.suspension().applies(&site.orgs);
        assert_eq!(shown.participating, full.participates(site));
        assert_eq!(
            shown.held,
            if active {
                full.effective(site)
            } else {
                Vec::new()
            }
        );
        assert_eq!(shown.owner, active && full.owns(site));
        assert_eq!(shown.administers, active && full.administers(site));
        assert_eq!(shown.role, full.role_for(site));
        assert_eq!(
            shown.admin_only,
            if active {
                full.admin_only(site)
            } else {
                Vec::new()
            }
        );
        assert_eq!(
            shown.admin_access,
            if active {
                full.admin_access(site)
            } else {
                None
            }
        );
        assert_eq!(
            shown.admin_place,
            if active { full.admin_place(site) } else { None }
        );
    }
    let expected: BTreeSet<_> = sites
        .iter()
        .filter(|site| {
            !principal.suspension().applies(&site.orgs) && !full.effective(site).is_empty()
        })
        .map(|site| site.id)
        .collect();
    let mut after = None;
    let mut listed = Vec::new();
    let mut ended = false;
    // The fixture owns exactly these providers; one-row pages exercise their continuation order.
    for _ in 0..=ids.len() {
        let page = resources::visible(
            &mut conn,
            &principal,
            ResourceKind::Provider,
            orch_authz::nodes::Page { after, limit: 1 },
            Policy::default(),
        )
        .await
        .unwrap();
        for shown in page.items {
            let site = sites
                .iter()
                .find(|site| site.id == shown.resource.id)
                .expect("only fixture providers are listed");
            assert_eq!(shown.participating, full.participates(site));
            assert_eq!(shown.held, full.effective(site));
            assert_eq!(shown.owner, full.owns(site));
            assert_eq!(shown.administers, full.administers(site));
            assert_eq!(shown.admin_only, full.admin_only(site));
            assert_eq!(shown.admin_access, full.admin_access(site));
            assert_eq!(shown.admin_place, full.admin_place(site));
            listed.push(shown.resource.id);
        }
        if let Some(next) = page.next {
            assert!(after.is_none_or(|previous| next > previous));
            after = Some(next);
        } else {
            ended = true;
            break;
        }
    }
    assert!(ended, "the bounded resource list must exhaust");
    assert_eq!(listed, expected.into_iter().collect::<Vec<_>>());
    ids.iter()
        .map(|id| {
            among
                .iter()
                .find(|shown| shown.object.id == *id)
                .unwrap()
                .participating
        })
        .collect()
}

#[tokio::test]
async fn resource_batches_keep_cross_home_participation_permissions_and_cursors() {
    let w = World::new().await;
    let other = orch_types::new_id();
    {
        let mut conn = w.conn().await;
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn, other, "org", None, "Other", w.boss,
        )
        .await
        .unwrap();
        insert_member(&mut conn, other, w.boss, other)
            .await
            .unwrap();
    }
    let mut ids = Vec::new();
    for (home, name) in [(w.root, "First provider"), (other, "Other provider")] {
        let mut conn = w.conn().await;
        let id =
            access_fixtures::insert_resource(&mut conn, "provider", Some(w.boss), Some(home), name)
                .await
                .unwrap();
        add_entry(
            &mut conn,
            &PageKey::new("provider", id, "live"),
            "any",
            "access.own",
            ("user", Some(w.boss), None),
            None,
            w.now.get(),
        )
        .await
        .unwrap();
        ids.push(id);
    }
    assert_eq!(
        compare_batched_participation(&w, Caller::session(w.boss), &ids).await,
        vec![true, true]
    );
    compare_batched_participation(&w, Caller::session(w.maker), &ids).await;
    let reader = w.user("batchreader").await;
    let group = w.group(w.root, "Batch readers", &[reader]).await;
    let mut grants = Vec::new();
    for id in &ids {
        let mut conn = w.conn().await;
        grants.push(
            add_entry(
                &mut conn,
                &PageKey::new("provider", *id, "live"),
                "any",
                "provider.view",
                ("group", Some(group), None),
                Some(w.now.get() + Duration::days(1)),
                w.now.get(),
            )
            .await
            .unwrap(),
        );
    }
    assert_eq!(
        compare_batched_participation(&w, Caller::session(reader), &ids).await,
        vec![true, true]
    );
    for subtree in [w.root, other] {
        compare_batched_participation(
            &w,
            Caller::token(reader, TokenGrant::new(Scopes::All, Some(subtree))),
            &ids,
        )
        .await;
    }
    {
        let mut conn = w.conn().await;
        sqlx::query("UPDATE permission_entries SET expires_at=$2 WHERE id=ANY($1)")
            .bind(&grants)
            .bind(w.now.get())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    assert_eq!(
        compare_batched_participation(&w, Caller::session(reader), &ids).await,
        vec![false, false]
    );
    let administrator = w.user("batchadmin").await;
    {
        let mut conn = w.conn().await;
        make_system_admin(&mut conn, administrator).await.unwrap();
    }
    assert_eq!(
        compare_batched_participation(
            &w,
            Caller::session(administrator).with_admin_mode(true),
            &ids
        )
        .await,
        vec![false, false]
    );
}
