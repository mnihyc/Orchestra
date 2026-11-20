//! Ordinary account and permission facts belong to one effect-free database snapshot.

use crate::places::{Who, join, make_org, make_project, person};
use crate::support::{Statements, World};
use orch_access_contract::{
    decisions::Decision,
    permissions::{Permission, Profile},
};
use orch_authz::engine::{PageRef, Store};
use orch_authz::{Act, Caller, Policy, Principal, read_facts};
use orch_db::queries::access::{self, PageKey, testing};
use orch_types::{Id, Time};

async fn fixture(expiring: bool) -> (World, Who, Who, Id, Id, Id) {
    let world = World::new().await;
    let owner = person(&world, "snapshot-owner").await;
    let reader = person(&world, "snapshot-reader").await;
    let root = make_org(&world, &owner, "Snapshot pages", "snapshot-pages")
        .await
        .base
        .id;
    join(&world, root, root, &reader, "viewer", &owner).await;
    let project = make_project(&world, &owner, root, "Private project")
        .await
        .base
        .id;
    let mut conn = world.conn().await;
    let page = PageKey::new("node", project, "live");
    testing::set_row(&mut conn, &page, "any", "project.view", false, false)
        .await
        .unwrap();
    testing::set_row(&mut conn, &page, "any", "project.manage", false, false)
        .await
        .unwrap();
    let entry = testing::add_entry(
        &mut conn,
        &page,
        "any",
        "project.view",
        ("user", Some(reader.id()), None),
        expiring.then(|| world.now() + chrono::Duration::seconds(1)),
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    (world, owner, reader, root, project, entry)
}

async fn may_view(conn: &mut orch_db::Conn, reader: Id, project: Id, at: Time) -> bool {
    let principal = Principal::load(conn, Caller::session(reader), at)
        .await
        .unwrap();
    matches!(
        orch_authz::nodes::check(
            conn,
            &principal,
            project,
            Permission::ProjectView,
            Act::Use,
            Policy::default()
        )
        .await
        .unwrap(),
        Decision::Allowed { .. }
    )
}

#[tokio::test]
async fn an_opening_reads_each_ancestor_and_environment_once_in_its_snapshot() {
    use orch_db::queries::{environment_facts, hierarchy};

    let (world, _, [org, _, _, thread, _]) = super::page_budgets::fixture(1, true).await;
    let mut other = world.conn().await;
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let path = hierarchy::with_ancestor_governance(conn, &[thread])
                .await
                .unwrap();
            let leaf = path.iter().find(|(row, _)| row.id == thread).unwrap();
            let environment = leaf.0.environment_id.unwrap();
            let (root, queries) =
                Statements::measure(hierarchy::with_ancestors(conn, &[org, org])).await;
            let root = root.unwrap();
            assert_eq!(root.len(), 1);
            assert_eq!(root[0].id, org);
            assert_eq!(queries, 0, "the opening already read this part of the path");

            let columns = environment_facts::access_profiles(conn, &[environment])
                .await
                .unwrap();
            assert_eq!(columns.len(), 1);
            let (machine, queries) =
                Statements::measure(environment_facts::of(conn, environment)).await;
            let machine = machine.unwrap().unwrap().machine_id.unwrap();
            assert_eq!(
                queries, 0,
                "a directory's access column already read its physical machine"
            );
            let (repeated, queries) = Statements::measure(environment_facts::access_profiles(
                conn,
                &[environment, environment],
            ))
            .await;
            assert_eq!(repeated.unwrap(), columns);
            assert_eq!(queries, 0);

            let (independent, queries) =
                Statements::measure(environment_facts::of(&mut other, environment)).await;
            assert_eq!(independent.unwrap().unwrap().machine_id, Some(machine));
            assert!(
                queries > 0,
                "a separate connection samples current machine facts"
            );
            let mut trial = access::begin_trial(conn).await.unwrap();
            let (again, queries) =
                Statements::measure(hierarchy::with_ancestors(&mut trial, &[thread])).await;
            assert!(again.unwrap().iter().any(|row| row.id == thread));
            assert!(queries > 0, "a trial rereads the hierarchy");
            let (again, queries) =
                Statements::measure(environment_facts::of(&mut trial, environment)).await;
            assert_eq!(again.unwrap().unwrap().machine_id, Some(machine));
            assert!(queries > 0, "a trial rereads the environment");
            access::end_trial(trial).await.unwrap();
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn opening_panels_read_the_same_bindings_and_settings_once() {
    use orch_db::queries::{resources, settings};

    let (world, _, [org, project, _, thread, _]) = super::page_budgets::fixture(1, true).await;
    let mut other = world.conn().await;
    let (env, agent): (Id, Id) =
        sqlx::query_as("SELECT environment_id,agent_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(&mut *other)
            .await
            .unwrap();
    // Separate native resource kinds may use the same UUID.
    sqlx::query(
        "INSERT INTO agents (id,name,owner_id,org_id,home_id,harness_kind,profile)
                 SELECT $1,'Other profile',owner_id,org_id,home_id,harness_kind,profile
                 FROM agents WHERE id=$2",
    )
    .bind(env)
    .bind(agent)
    .execute(&mut *other)
    .await
    .unwrap();
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let missing = orch_types::new_id();
            let original = resources::resource_facts(conn, &[env, agent, missing])
                .await
                .unwrap();
            assert_eq!(original.len(), 3);
            let (subset, reads) =
                Statements::measure(resources::resource_facts(conn, &[missing, env, env])).await;
            let mut kinds: Vec<_> = subset.unwrap().into_iter().map(|row| row.kind).collect();
            kinds.sort();
            assert_eq!(kinds, ["agent", "environment"]);
            assert_eq!(
                reads, 0,
                "each kind and the absent resource were already read"
            );

            let environment = settings::thread_environment(conn, thread)
                .await
                .unwrap()
                .unwrap();
            let (many, reads) =
                Statements::measure(settings::thread_environments(conn, &[thread, thread])).await;
            assert_eq!(many.unwrap(), [(thread, environment)]);
            assert_eq!(
                reads, 0,
                "the parameter and settings panes share the template facts"
            );

            let all = settings::rows_on_chain(conn, &[org, project, thread], None)
                .await
                .unwrap();
            let keys = ["agent.model".to_owned()];
            let expected: Vec<_> = all
                .into_iter()
                .filter(|row| {
                    row.key == keys[0] && (row.node_id.is_none() || row.node_id == Some(thread))
                })
                .collect();
            assert!(expected.iter().any(|row| row.node_id == Some(thread)));
            let (subset, reads) = Statements::measure(settings::rows_on_chain(
                conn,
                &[thread, thread],
                Some(&keys),
            ))
            .await;
            let subset = subset.unwrap();
            assert_eq!(subset.len(), expected.len());
            assert!(expected.iter().all(|row| subset.contains(row)));
            assert_eq!(
                reads, 0,
                "the earlier chain includes each requested node and key"
            );
            let (_, reads) =
                Statements::measure(settings::rows_on_chain(&mut other, &[thread], Some(&keys)))
                    .await;
            assert!(reads > 0, "another connection reads its own rows");
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn opening_controls_share_the_thread_workspace_model_and_live_owner_rows() {
    use orch_db::queries::{environments::thread_engine, settings, threads};

    let (world, _, [_, _, _, thread, _]) = super::page_budgets::fixture(1, true).await;
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let row = threads::thread(conn, thread).await.unwrap().unwrap();
            let env = row.environment_id.unwrap();
            let workspace = thread_engine::environment(conn, env)
                .await
                .unwrap()
                .unwrap();
            let model = settings::thread_model_parameters(conn, thread, "scale-model")
                .await
                .unwrap()
                .unwrap();
            let actual = PageKey::new("environment", env, "live");
            let other_kind = PageKey::new("machine", env, "live");
            let owners = access::owner_entries(conn, std::slice::from_ref(&actual))
                .await
                .unwrap();
            assert_eq!(owners.len(), 1);
            let all = access::owner_entries(conn, &[other_kind.clone(), actual.clone()])
                .await
                .unwrap();
            assert_eq!(all, owners);
            let ((), reads) = Statements::measure(async {
                assert_eq!(threads::thread(conn, thread).await.unwrap(), Some(row));
                assert_eq!(
                    thread_engine::environment(conn, env).await.unwrap(),
                    Some(workspace)
                );
                assert_eq!(
                    settings::thread_model_parameters(conn, thread, "scale-model")
                        .await
                        .unwrap(),
                    Some(model)
                );
                assert!(
                    access::owner_entries(conn, &[other_kind])
                        .await
                        .unwrap()
                        .is_empty()
                );
                assert_eq!(
                    access::owner_entries(conn, &[actual]).await.unwrap(),
                    owners
                );
            })
            .await;
            assert_eq!(reads, 0, "another pane reads the same observed rows");
            let (absent, reads) =
                Statements::measure(settings::thread_model_parameters(conn, thread, "unlisted"))
                    .await;
            assert!(absent.unwrap().is_none());
            assert!(reads > 0, "another model has its own parameters");
            let (absent, reads) =
                Statements::measure(settings::thread_model_parameters(conn, thread, "unlisted"))
                    .await;
            assert!(absent.unwrap().is_none());
            assert_eq!(reads, 0);
            let mut trial = access::begin_trial(conn).await.unwrap();
            sqlx::query("UPDATE threads SET workspace_dir='other' WHERE node_id=$1")
                .bind(thread)
                .execute(&mut *trial)
                .await
                .unwrap();
            assert_eq!(
                threads::thread(&mut trial, thread)
                    .await
                    .unwrap()
                    .unwrap()
                    .workspace_dir,
                "other"
            );
            access::end_trial(trial).await.unwrap();
            assert_eq!(
                threads::thread(conn, thread)
                    .await
                    .unwrap()
                    .unwrap()
                    .workspace_dir,
                "work"
            );
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn the_opening_can_read_a_setting_without_resolving_unrelated_model_defaults() {
    use orch_api_base::routes::settings::readers;
    let (world, reader, [_, _, _, thread, _]) = super::page_budgets::fixture(1, true).await;
    let mut conn = world.conn().await;
    let principal = Principal::load(
        &mut conn,
        Caller::session(reader.id()),
        Time::new(world.now()),
    )
    .await
    .unwrap();
    let full = readers::prepare_settings(&mut conn, &principal, thread)
        .await
        .unwrap();
    let names = readers::setting_source_nodes(&mut conn, &principal, thread)
        .await
        .unwrap();
    assert_eq!(names.into_iter().collect::<Vec<_>>(), full.source_nodes());
    let keys = ["agent.model", "agent.model", "missing.fixture.key"];
    let (narrow, work) = Statements::queries(readers::resolved_views(
        &mut conn,
        &principal,
        thread,
        Some(&keys),
    ))
    .await;
    let expected: Vec<_> = full
        .views
        .into_iter()
        .filter(|(view, _)| view.key() == keys[0])
        .collect();
    assert_eq!(expected.len(), 1);
    assert_eq!(
        narrow.unwrap(),
        expected,
        "the value, provenance and write hint match the panel"
    );
    assert!(work.totals.statements > 0);
    assert!(
        !work
            .statements
            .keys()
            .any(|sql| sql.contains("provider_option_layers(pm.provider_id")),
        "reading the selected model name alone does not need its effort defaults"
    );
}

#[tokio::test]
async fn opening_panels_share_role_rows_and_still_observe_their_expiry() {
    let (world, owner, reader, org, project, _) = fixture(false).await;
    let now = world.now();
    let end = now + chrono::Duration::seconds(1);
    let role = testing::give_role(
        &mut *world.conn().await,
        project,
        "user",
        reader.id(),
        "viewer",
        owner.id(),
        Some(end),
    )
    .await
    .unwrap();
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let later = access::roles_in_places(conn, &[project], end)
                .await
                .unwrap();
            assert!(later.iter().all(|row| row.id != role));
            let earlier = access::roles_in_places(conn, &[org, project], now)
                .await
                .unwrap();
            assert!(earlier.iter().any(|row| row.id == role));
            let (again, reads) =
                Statements::measure(access::roles_in_places(conn, &[project, org, project], end))
                    .await;
            let expected: Vec<_> = earlier
                .into_iter()
                .filter(|row| row.expires_at.is_none_or(|expires| expires > end))
                .collect();
            assert_eq!(again.unwrap(), expected);
            assert_eq!(
                reads, 0,
                "later decisions check expiry on the same ordered rows"
            );
            let mut trial = access::begin_trial(conn).await.unwrap();
            let (_, reads) =
                Statements::measure(access::roles_in_places(&mut trial, &[org], end)).await;
            assert!(reads > 0);
            access::end_trial(trial).await.unwrap();
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn one_page_reuses_account_and_permission_rows_without_lending_unrequested_permissions() {
    let (world, owner, reader, _root, project, entry) = fixture(false).await;
    let reader = reader.id();
    let at = Time::new(world.now());
    let mut other = world.conn().await;
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let (first, first_queries) =
                Statements::measure(Principal::load(conn, Caller::session(reader), at)).await;
            let (second, repeated_queries) =
                Statements::measure(Principal::load(conn, Caller::session(reader), at)).await;
            assert!(first_queries > 0);
            assert_eq!(
                repeated_queries, 0,
                "the same snapshot reads its ordinary account facts once"
            );
            assert_eq!(first.unwrap().member_of(), second.unwrap().member_of());
            let (_, other_queries) =
                Statements::measure(Principal::load(&mut other, Caller::session(reader), at)).await;
            assert!(
                other_queries > 0,
                "another connection reads its own account facts"
            );
            let page = PageRef::live(orch_access_contract::pages::StoredKind::Node, project);
            let (_, first_queries) =
                Statements::measure(Store::load(conn, &[page], &[Permission::ProjectView])).await;
            let (second, repeated_queries) =
                Statements::measure(Store::load(conn, &[page], &[Permission::ProjectView])).await;
            assert_eq!(first_queries, 2);
            assert_eq!(
                repeated_queries, 0,
                "one snapshot loads these stored rows and entries once"
            );
            assert_eq!(
                second
                    .unwrap()
                    .row(page, Profile::Any, Permission::ProjectView)
                    .unwrap()
                    .entries
                    .len(),
                1
            );
            Store::load(conn, &[page], &[Permission::ProjectManage])
                .await
                .unwrap();
            let subset = Store::load(conn, &[page], &[Permission::ProjectView])
                .await
                .unwrap();
            assert!(
                subset
                    .row(page, Profile::Any, Permission::ProjectManage)
                    .is_none(),
                "a later frame gets only its requested permissions"
            );
        })
    })
    .await;
    access::remove_entry(&mut tx, entry, owner.id(), at.get())
        .await
        .unwrap();
    assert!(
        !may_view(&mut tx, reader, project, at).await,
        "the completed read scope leaves no facts for a later write"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn a_new_page_snapshot_sees_revocation_and_group_membership_after_an_earlier_read() {
    let (world, owner, reader, root, project, entry) = fixture(false).await;
    let group = testing::insert_group(&mut *world.conn().await, root, "Later members", owner.id())
        .await
        .unwrap();
    let at = Time::new(world.now());
    let reader = reader.id();
    let owner = owner.id();
    let database = world.db.pool.clone();
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let before = Principal::load(conn, Caller::session(reader), at)
                .await
                .unwrap();
            assert!(!before.groups().contains(&group));
            assert!(may_view(conn, reader, project, at).await);
            let mut changed = database.begin().await.unwrap();
            testing::add_to_group(&mut changed, group, reader)
                .await
                .unwrap();
            access::remove_entry(&mut changed, entry, owner, at.get())
                .await
                .unwrap();
            access::bump_epoch(&mut changed).await.unwrap();
            changed.commit().await.unwrap();
            let same = Principal::load(conn, Caller::session(reader), at)
                .await
                .unwrap();
            assert!(
                !same.groups().contains(&group),
                "the current snapshot stays coherent"
            );
            assert!(may_view(conn, reader, project, at).await);
        })
    })
    .await;
    tx.commit().await.unwrap();
    let mut fresh = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut fresh, move |conn| {
        Box::pin(async move {
            let current = Principal::load(conn, Caller::session(reader), at)
                .await
                .unwrap();
            assert!(
                current.groups().contains(&group),
                "a fresh attempt rereads changed membership"
            );
            assert!(
                !may_view(conn, reader, project, at).await,
                "revocation invalidates the next snapshot"
            );
        })
    })
    .await;
    fresh.commit().await.unwrap();
}

#[tokio::test]
async fn reused_permission_rows_still_expire_inside_a_snapshot_when_its_clock_moves() {
    let (world, _owner, reader, _root, project, _entry) = fixture(true).await;
    let reader = reader.id();
    let at = Time::new(world.now());
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            assert!(may_view(conn, reader, project, at).await);
            let later = Time::new(at.get() + chrono::Duration::seconds(2));
            let (_, queries) =
                Statements::measure(Principal::load(conn, Caller::session(reader), later)).await;
            assert!(
                queries > 0,
                "an expired holder interval is read again at the new time"
            );
            assert!(
                !may_view(conn, reader, project, later).await,
                "stored entries are evaluated at the new time"
            );
        })
    })
    .await;
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_preview_reads_its_trial_changes_and_never_keeps_them_after_rollback() {
    let (world, owner, reader, _root, project, entry) = fixture(false).await;
    let at = Time::new(world.now());
    let owner = owner.id();
    let reader = reader.id();
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            assert!(may_view(conn, reader, project, at).await);
            let mut trial = access::begin_trial(conn).await.unwrap();
            access::remove_entry(&mut trial, entry, owner, at.get())
                .await
                .unwrap();
            assert!(
                !may_view(&mut trial, reader, project, at).await,
                "a preview sees its tentative revocation"
            );
            access::end_trial(trial).await.unwrap();
            assert!(
                may_view(conn, reader, project, at).await,
                "rollback leaves no tentative permission facts behind"
            );
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn ordinary_write_transactions_never_reuse_permission_rows_after_their_own_changes() {
    let (world, owner, reader, _root, project, entry) = fixture(false).await;
    let at = Time::new(world.now());
    let owner = owner.id();
    let reader = reader.id();
    let mut tx = world.app.app.db.begin().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            assert!(may_view(conn, reader, project, at).await);
            access::remove_entry(conn, entry, owner, at.get())
                .await
                .unwrap();
            assert!(
                !may_view(conn, reader, project, at).await,
                "a write sees its own current permission rows"
            );
        })
    })
    .await;
    tx.rollback().await.unwrap();
}
