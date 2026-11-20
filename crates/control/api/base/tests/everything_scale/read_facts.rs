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
