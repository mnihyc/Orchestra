//! Properties that hold of every world: taking something away never gives anybody more, a lock gives the same holders below it, and a token never has what its user lacks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use std::collections::BTreeSet;

use chrono::Duration;
use orch_access_contract::decisions::Scopes;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_access_contract::roles::{NodeKind, Role};
use orch_authz::engine::{Frame, Graph};
use orch_authz::{Caller, Policy, Principal, TokenGrant};
use orch_db::queries::access::testing::{
    add_entry, give_role, insert_member, make_system_admin, name_owner, set_row, take_role_away,
};
use orch_db::queries::access::{PageKey, SYSTEM_ID, remove_entry};
use orch_types::Id;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use support::World;

/// The worlds each property is tried in: enough that a rule broken in one
/// corner shows, few enough to run in seconds.
const WORLDS: u64 = 12;

type Held = BTreeSet<(Id, Id, Permission)>;

#[tokio::test]
async fn selected_reads_keep_owner_rows_locks_archive_and_token_boundaries() {
    let requested = [
        Permission::OrgView,
        Permission::ProjectView,
        Permission::TopicView,
        Permission::ThreadView,
        Permission::ChatView,
        Permission::DocView,
        Permission::ThreadInteract,
        Permission::SettingsManage,
        Permission::AccessExternal,
        Permission::AccessOwn,
    ];
    for seed in [710, 711] {
        let scene = Scene::new(seed).await;
        let ids: Vec<_> = scene.nodes.iter().map(|(id, _)| *id).collect();
        for archived in [false, true] {
            sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
                .bind(scene.w.topic)
                .bind(archived.then_some(scene.w.now.get()))
                .execute(&scene.w.db.pool)
                .await
                .unwrap();
            for user in scene
                .users
                .iter()
                .copied()
                .chain([scene.w.boss, scene.w.maker])
            {
                let token = TokenGrant::from_stored(
                    &["thread.view".to_owned(), "chat.view".to_owned()],
                    Some(scene.w.topic),
                )
                .unwrap();
                let writing_token = TokenGrant::from_stored(
                    &["thread.interact".to_owned(), "doc.edit".to_owned()],
                    Some(scene.w.topic),
                )
                .unwrap();
                for caller in [
                    Caller::session(user),
                    Caller::session(user).with_admin_mode(true),
                    Caller::token(user, token),
                    Caller::token(user, writing_token),
                ] {
                    let mut conn = scene.w.conn().await;
                    let principal = Principal::load(&mut conn, caller, scene.w.now)
                        .await
                        .unwrap();
                    let (frame, sites) = support::frame_for(&mut conn, &principal, &ids).await;
                    for site in sites {
                        let known = site
                            .kind
                            .permissions()
                            .any(|permission| !frame.ways(&site, permission).is_empty());
                        assert_eq!(
                            frame.holds_any(&site),
                            known,
                            "names retain every ordinary way and survive archive within the same credential bounds"
                        );
                        let complete = orch_access_contract::decisions::effective(
                            site.kind,
                            &frame.standing(&site),
                            false,
                            |permission| frame.named(&site, permission),
                        );
                        let originally_known = principal.caller().reaches(site.path.as_deref())
                            && complete
                                .iter()
                                .any(|held| principal.caller().permits(held.permission));
                        assert_eq!(
                            frame.holds_any(&site),
                            originally_known,
                            "the complete original name decision is unchanged"
                        );
                        for owned in [false, true] {
                            let expected: Vec<_> = if owned {
                                frame.effective_owned(&site)
                            } else {
                                frame.effective(&site)
                            }
                            .into_iter()
                            .filter(|held| requested.contains(&held.permission))
                            .collect();
                            let mut selected = frame.effective_for(&site, &requested, owned);
                            selected.sort_by_key(|held| held.permission);
                            let mut expected = expected;
                            expected.sort_by_key(|held| held.permission);
                            for permission in requested {
                                assert_eq!(
                                    frame.has_permission(&site, permission, owned),
                                    expected.iter().any(|held| held.permission == permission),
                                    "boolean reads retain the full decision's bounds and covering rows"
                                );
                            }
                            assert_eq!(
                                selected, expected,
                                "the same rows, levels, archive and credential bounds apply"
                            );
                        }
                    }
                }
            }
        }
    }
}

struct Scene {
    w: World,
    rng: StdRng,
    users: Vec<Id>,
    admin: Id,
    places: Vec<Id>,
    nodes: Vec<(Id, NodeKind)>,
    groups: Vec<Id>,
}

fn pick<T: Copy>(rng: &mut StdRng, from: &[T]) -> T {
    from[rng.random_range(0..from.len())]
}

impl Scene {
    async fn new(seed: u64) -> Self {
        let w = World::new().await;
        let mut rng = StdRng::seed_from_u64(seed);
        let d1 = w.node("org", w.root, "D1").await;
        let d2 = w.node("org", d1, "D2").await;
        let orgs = [w.root, d1, d2];
        let mut projects = vec![w.project];
        for index in 0..2 {
            let parent = pick(&mut rng, &orgs);
            projects.push(w.node("project", parent, &format!("P{index}")).await);
        }
        let mut topics = vec![w.topic];
        for index in 0..2 {
            let parent = pick(&mut rng, &projects);
            topics.push(w.node("topic", parent, &format!("T{index}")).await);
        }
        let mut threads = vec![w.thread];
        for index in 0..4 {
            let parent = pick(&mut rng, &topics);
            threads.push(w.node("thread", parent, &format!("H{index}")).await);
        }
        let mut users = Vec::new();
        for index in 0..5 {
            let user = w.user(&format!("user{index}")).await;
            if rng.random_bool(0.6) {
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
            let org = pick(&mut rng, &orgs);
            let members: Vec<Id> = users
                .iter()
                .copied()
                .filter(|_| rng.random_bool(0.4))
                .collect();
            groups.push(w.group(org, &format!("G{index}"), &members).await);
        }
        let mut places: Vec<Id> = orgs.to_vec();
        places.extend(&projects);
        places.extend(&topics);
        let mut nodes: Vec<(Id, NodeKind)> = orgs.iter().map(|id| (*id, NodeKind::Org)).collect();
        nodes.extend(projects.iter().map(|id| (*id, NodeKind::Project)));
        nodes.extend(topics.iter().map(|id| (*id, NodeKind::Topic)));
        nodes.extend(threads.iter().map(|id| (*id, NodeKind::Thread)));
        let mut scene = Self {
            w,
            rng,
            users,
            admin,
            places,
            nodes,
            groups,
        };
        scene.populate().await;
        scene
    }

    fn holder(&mut self) -> (&'static str, Option<Id>, Option<&'static str>) {
        let role = pick(&mut self.rng, Role::ALL).as_str();
        match self.rng.random_range(0..4) {
            0 => ("role", None, Some(role)),
            1 => ("role", Some(pick(&mut self.rng, &self.places)), Some(role)),
            2 => ("user", Some(pick(&mut self.rng, &self.users)), None),
            _ => ("group", Some(pick(&mut self.rng, &self.groups)), None),
        }
    }

    fn expiry(&mut self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self.rng.random_range(0..7) {
            0 => Some(self.w.now.get() - Duration::hours(1)),
            1 => Some(self.w.now.get() + Duration::days(1)),
            _ => None,
        }
    }

    async fn populate(&mut self) {
        let w_now = self.w.now.get();
        // Roles in places.
        for _ in 0..10 {
            let place = pick(&mut self.rng, &self.places);
            let role = pick(&mut self.rng, Role::ALL).as_str();
            let (kind, subject) = match self.rng.random_range(0..3) {
                0 => ("user", pick(&mut self.rng, &self.users)),
                1 => ("group", pick(&mut self.rng, &self.groups)),
                _ => ("members", pick(&mut self.rng, &self.places[..3])),
            };
            let expires = self.expiry();
            let mut conn = self.w.conn().await;
            if let Ok(id) =
                give_role(&mut conn, place, kind, subject, role, self.w.boss, expires).await
                && self.rng.random_bool(0.15)
            {
                take_role_away(&mut conn, id, self.w.boss, w_now)
                    .await
                    .unwrap();
            }
        }
        // Rows and entries on pages, and on defaults pages.
        for _ in 0..30 {
            let (node, kind) = pick(&mut self.rng, &self.nodes);
            let permissions: Vec<Permission> = ObjectKind::from(kind)
                .permissions()
                .filter(|permission| *permission != Permission::AccessOwn)
                .collect();
            let permission = pick(&mut self.rng, &permissions);
            let live = self.rng.random_bool(0.6);
            let place = pick(&mut self.rng, &self.places);
            let (page, profile) = if live {
                (PageKey::new("node", node, "live"), "any")
            } else if permission.profiles() {
                (
                    PageKey::new("node", place, "defaults"),
                    if self.rng.random_bool(0.5) {
                        "managed"
                    } else {
                        "connected"
                    },
                )
            } else {
                (PageKey::new("node", place, "defaults"), "any")
            };
            if !live
                && !ObjectKind::ALL.iter().any(|object| {
                    permission.exists_on(*object) && object.made_in(place_kind(&self.nodes, place))
                })
            {
                continue;
            }
            let holder = self.holder();
            let expires = self.expiry();
            let inherit = !self.rng.random_bool(0.35);
            let mut conn = self.w.conn().await;
            if !inherit {
                set_row(&mut conn, &page, profile, permission.as_str(), false, false)
                    .await
                    .unwrap();
            }
            if let Ok(id) = add_entry(
                &mut conn,
                &page,
                profile,
                permission.as_str(),
                holder,
                expires,
                w_now,
            )
            .await
                && self.rng.random_bool(0.12)
            {
                remove_entry(&mut conn, id, self.w.boss, w_now)
                    .await
                    .unwrap();
            }
        }
        // More owners.
        for _ in 0..3 {
            let (node, _) = pick(&mut self.rng, &self.nodes);
            let user = pick(&mut self.rng, &self.users);
            let mut conn = self.w.conn().await;
            let _ = name_owner(&mut conn, node, ("user", user), w_now).await;
        }
    }

    /// Everything each person holds on each node, for callers made by `caller`.
    async fn held(&self, caller: impl Fn(Id) -> Caller) -> Held {
        let ids: Vec<Id> = self.nodes.iter().map(|(id, _)| *id).collect();
        let everything: Vec<Permission> = Permission::ALL.to_vec();
        let mut held = Held::new();
        for user in &self.users {
            let mut conn = self.w.conn().await;
            let principal = Principal::load(&mut conn, caller(*user), self.w.now)
                .await
                .unwrap();
            let graph = Graph::load(&mut conn, &ids).await.unwrap();
            let sites: Vec<_> = ids.iter().filter_map(|id| graph.site_of(*id)).collect();
            let frame = Frame::load(
                &mut conn,
                &principal,
                Policy::default(),
                graph,
                &sites,
                &everything,
            )
            .await
            .unwrap();
            for site in &sites {
                for each in frame.effective(site) {
                    held.insert((*user, site.id, each.permission));
                }
            }
        }
        held
    }

    async fn sessions(&self) -> Held {
        self.held(Caller::session).await
    }

    async fn some_entry(&mut self) -> Option<Id> {
        let entries: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM permission_entries WHERE removed_at IS NULL AND permission <> 'access.own' ORDER BY id",
        )
        .fetch_all(&self.w.db.pool)
        .await
        .unwrap();
        (!entries.is_empty()).then(|| pick(&mut self.rng, &entries))
    }
}

fn place_kind(nodes: &[(Id, NodeKind)], place: Id) -> orch_access_contract::roles::PlaceKind {
    nodes
        .iter()
        .find(|(id, _)| *id == place)
        .and_then(|(_, kind)| kind.place())
        .unwrap_or(orch_access_contract::roles::PlaceKind::Org)
}

fn gained(before: &Held, after: &Held) -> Vec<(Id, Id, Permission)> {
    after.difference(before).copied().collect()
}

#[tokio::test]
async fn nobodys_permissions_grow_when_an_entry_is_removed_or_expires() {
    let mut took_something_away = 0_usize;
    for seed in 0..WORLDS {
        let mut scene = Scene::new(seed).await;
        let before = scene.sessions().await;
        assert!(
            !before.is_empty(),
            "seed {seed}: a world where nobody holds anything tests nothing"
        );
        for round in 0..4 {
            let Some(entry) = scene.some_entry().await else {
                break;
            };
            let now = scene.w.now.get();
            let how = if round % 2 == 0 {
                let mut conn = scene.w.conn().await;
                remove_entry(&mut conn, entry, scene.w.boss, now)
                    .await
                    .unwrap();
                "removed"
            } else {
                sqlx::query("UPDATE permission_entries SET expires_at = $2 WHERE id = $1")
                    .bind(entry)
                    .bind(now - Duration::seconds(1))
                    .execute(&scene.w.db.pool)
                    .await
                    .unwrap();
                "expired"
            };
            let after = scene.sessions().await;
            assert!(
                gained(&before, &after).is_empty(),
                "seed {seed}: an entry {how} gave {:?}",
                gained(&before, &after)
            );
            // Taking away again from what is left still only takes away.
            let again = scene.sessions().await;
            assert_eq!(after, again);
            took_something_away += usize::from(after.len() < before.len());
        }
    }
    assert!(
        took_something_away > 0,
        "the worlds are such that removing an entry sometimes takes something away"
    );
}

#[tokio::test]
async fn cutting_a_row_loose_never_grows_anyones_permissions() {
    let mut took_something_away = 0_usize;
    for seed in 0..WORLDS {
        let mut scene = Scene::new(seed + 100).await;
        let before = scene.sessions().await;
        assert!(!before.is_empty());
        for round in 0..4 {
            let (node, kind) = pick(&mut scene.rng, &scene.nodes.clone());
            let permissions: Vec<Permission> = ObjectKind::from(kind)
                .permissions()
                .filter(|permission| *permission != Permission::AccessOwn)
                .collect();
            let permission = pick(&mut scene.rng, &permissions);
            let place = pick(&mut scene.rng, &scene.places.clone());
            let mut conn = scene.w.conn().await;
            // A page's own row, and then the row of a defaults page.
            if round % 2 == 0 {
                set_row(
                    &mut conn,
                    &PageKey::new("node", node, "live"),
                    "any",
                    permission.as_str(),
                    false,
                    false,
                )
                .await
                .unwrap();
            } else if permission.profiles() {
                for profile in ["managed", "connected"] {
                    set_row(
                        &mut conn,
                        &PageKey::new("node", place, "defaults"),
                        profile,
                        permission.as_str(),
                        false,
                        false,
                    )
                    .await
                    .unwrap();
                }
            } else {
                set_row(
                    &mut conn,
                    &PageKey::new("node", place, "defaults"),
                    "any",
                    permission.as_str(),
                    false,
                    false,
                )
                .await
                .unwrap();
            }
            drop(conn);
            let after = scene.sessions().await;
            assert!(
                gained(&before, &after).is_empty(),
                "seed {seed}: cutting {permission} loose gave {:?}",
                gained(&before, &after)
            );
            took_something_away += usize::from(after.len() < before.len());
        }
    }
    assert!(
        took_something_away > 0,
        "the worlds are such that cutting a row loose sometimes takes something away"
    );
}

#[tokio::test]
async fn a_locked_row_gives_the_same_holders_on_every_page_below_it() {
    for seed in 0..WORLDS {
        let mut scene = Scene::new(seed + 200).await;
        let root = scene.w.root;
        for permission in [
            Permission::ThreadInteract,
            Permission::ThreadManage,
            Permission::ThreadView,
            Permission::ThreadFork,
        ] {
            // The pages below differ: some added to the row, some cut it loose.
            for (node, kind) in scene.nodes.clone() {
                if kind == NodeKind::Thread && scene.rng.random_bool(0.6) {
                    let holder = scene.holder();
                    let cut = scene.rng.random_bool(0.5);
                    let mut conn = scene.w.conn().await;
                    if cut {
                        set_row(
                            &mut conn,
                            &PageKey::new("node", node, "live"),
                            "any",
                            permission.as_str(),
                            false,
                            false,
                        )
                        .await
                        .unwrap();
                    }
                    let _ = add_entry(
                        &mut conn,
                        &PageKey::new("node", node, "live"),
                        "any",
                        permission.as_str(),
                        holder,
                        None,
                        scene.w.now.get(),
                    )
                    .await;
                }
            }
            // The root org locks it with its own holders.
            {
                let mut conn = scene.w.conn().await;
                let page = PageKey::new("node", root, "defaults");
                let _ = add_entry(
                    &mut conn,
                    &page,
                    "managed",
                    permission.as_str(),
                    ("role", None, Some("developer")),
                    None,
                    scene.w.now.get(),
                )
                .await;
                set_row(&mut conn, &page, "managed", permission.as_str(), true, true)
                    .await
                    .unwrap();
            }
            let ids: Vec<Id> = scene
                .nodes
                .iter()
                .filter(|(_, kind)| *kind == NodeKind::Thread)
                .map(|(id, _)| *id)
                .collect();
            let principal = scene.w.principal(Caller::session(scene.w.boss)).await;
            let mut conn = scene.w.conn().await;
            let graph = Graph::load(&mut conn, &ids).await.unwrap();
            let sites: Vec<_> = ids.iter().filter_map(|id| graph.site_of(*id)).collect();
            let frame = Frame::load(
                &mut conn,
                &principal,
                Policy::default(),
                graph,
                &sites,
                &[permission],
            )
            .await
            .unwrap();
            let holders_of = |site| -> Vec<String> {
                let mut holders: Vec<String> = frame
                    .chain(site, permission)
                    .holders(principal.now())
                    .into_iter()
                    .map(|named| serde_json::to_string(&named.entry.holder).unwrap())
                    .collect();
                holders.sort();
                holders
            };
            let first = holders_of(&sites[0]);
            assert!(!first.is_empty());
            for site in &sites {
                assert_eq!(
                    holders_of(site),
                    first,
                    "seed {seed}: {permission} on {}",
                    site.name
                );
            }
            // And the lock is undone before the next permission.
            let mut conn = scene.w.conn().await;
            set_row(
                &mut conn,
                &PageKey::new("node", root, "defaults"),
                "managed",
                permission.as_str(),
                true,
                false,
            )
            .await
            .unwrap();
        }
    }
}

#[tokio::test]
async fn a_token_never_has_a_permission_its_user_lacks() {
    for seed in 0..WORLDS {
        let mut scene = Scene::new(seed + 300).await;
        let sessions = scene.sessions().await;
        let everything = scene
            .held(|user| Caller::token(user, TokenGrant::everything()))
            .await;
        assert!(
            everything.is_subset(&sessions),
            "seed {seed}: a token with every scope gave {:?}",
            everything.difference(&sessions).collect::<Vec<_>>()
        );
        // The system admin holds Admin everywhere in a session, and with a token only what their own roles give.
        let administrative = scene
            .held(|user| Caller::session(user).with_admin_mode(true))
            .await;
        let admin_session = administrative
            .iter()
            .filter(|(user, ..)| *user == scene.admin)
            .count();
        let admin_token = everything
            .iter()
            .filter(|(user, ..)| *user == scene.admin)
            .count();
        assert!(
            admin_token < admin_session,
            "seed {seed}: a token carries none of the system admin"
        );
        for round in 0..3 {
            let all = Permission::ALL;
            let scopes: Vec<String> = all
                .iter()
                .filter(|_| scene.rng.random_bool(0.4))
                .map(|permission| permission.as_str().to_owned())
                .collect();
            let subtree = (round > 0).then(|| pick(&mut scene.rng, &scene.places.clone()));
            let grant = TokenGrant::from_stored(&scopes, subtree).unwrap();
            let narrowed = scene.held(|user| Caller::token(user, grant.clone())).await;
            assert!(
                narrowed.is_subset(&everything),
                "seed {seed}: a narrowed token gave more"
            );
            // Exactly what its user holds, cut to its scopes and its subtree.
            let paths: std::collections::HashMap<Id, Vec<Id>> = {
                let mut conn = scene.w.conn().await;
                let ids: Vec<Id> = scene.nodes.iter().map(|(id, _)| *id).collect();
                let graph = Graph::load(&mut conn, &ids).await.unwrap();
                ids.iter()
                    .filter_map(|id| graph.get(*id).map(|node| (*id, node.path.clone())))
                    .collect()
            };
            let expected: Held = everything
                .iter()
                .filter(|(_, _, permission)| {
                    matches!(grant.scopes(), Scopes::All) || grant.carries(*permission)
                })
                .filter(|(_, node, _)| {
                    grant
                        .subtree()
                        .is_none_or(|root| paths[node].contains(&root))
                })
                .copied()
                .collect();
            assert_eq!(
                narrowed, expected,
                "seed {seed}, scopes {scopes:?}, subtree {subtree:?}"
            );
        }
        let _ = (Holder::Everyone, SYSTEM_ID);
    }
}
