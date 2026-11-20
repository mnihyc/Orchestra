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

#[tokio::test]
async fn opening_a_thread_reads_its_machine_and_placement_once_and_a_preview_sees_its_changes() {
    use orch_access_contract::permissions::Placement;
    use orch_db::queries::environment_facts::{access_profiles, of};
    let (world, _reader, [_, _, _, thread, _]) = super::page_budgets::fixture(1, true).await;
    let mut other = world.conn().await;
    let environment: Id = sqlx::query_scalar("SELECT environment_id FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(&mut *other)
        .await
        .unwrap();
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let (first, first_reads) = Statements::measure(of(conn, environment)).await;
            let machine = first.unwrap().unwrap();
            assert!(first_reads > 0);
            let profile = access_profiles(conn, &[environment]).await.unwrap();
            assert_eq!(profile, [(environment, Placement::Connected)]);
            let (again, machine_reads) = Statements::measure(of(conn, environment)).await;
            let (again_profile, placement_reads) =
                Statements::measure(access_profiles(conn, &[environment])).await;
            assert_eq!(again.unwrap(), Some(machine.clone()));
            assert_eq!(again_profile.unwrap(), profile);
            assert_eq!((machine_reads, placement_reads), (0, 0));
            let (_, other_reads) = Statements::measure(of(&mut other, environment)).await;
            assert!(
                other_reads > 0,
                "another connection reads the actual machine"
            );

            let mut trial = access::begin_trial(conn).await.unwrap();
            sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',$2) WHERE id=$1")
                .bind(machine.machine_id.unwrap())
                .bind(serde_json::json!({"policy.delegated":["environment.*"]}))
                .execute(&mut *trial)
                .await
                .unwrap();
            let changed = of(&mut trial, environment).await.unwrap().unwrap();
            assert_ne!(changed.policy, machine.policy);
            assert_eq!(
                access_profiles(&mut trial, &[environment]).await.unwrap(),
                [(environment, Placement::Managed)]
            );
            access::end_trial(trial).await.unwrap();
            assert_eq!(of(conn, environment).await.unwrap(), Some(machine));
            assert_eq!(
                access_profiles(conn, &[environment]).await.unwrap(),
                profile
            );
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn thread_status_is_read_once_for_the_same_thread_reader_and_time() {
    use orch_db::queries::source::thread_display_facts;
    let (world, reader, [_, _, _, thread, _]) = super::page_budgets::fixture(1, true).await;
    let reader = Some(reader.id());
    let at = Time::new(world.now());
    let later = Time::new(world.now() + chrono::Duration::seconds(1));
    let mut other = world.conn().await;
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let (first, calls) =
                Statements::measure(thread_display_facts(conn, &[thread], reader, at)).await;
            let first = first.unwrap();
            assert_eq!(first[0].id, thread);
            assert!(calls > 0);
            let (again, calls) =
                Statements::measure(thread_display_facts(conn, &[thread], reader, at)).await;
            assert_eq!(again.unwrap()[0].read_seq, first[0].read_seq);
            assert_eq!(calls, 0);
            for (person, clock) in [(None, at), (None, later)] {
                let (different, calls) =
                    Statements::measure(thread_display_facts(conn, &[thread], person, clock)).await;
                assert_eq!(different.unwrap()[0].id, thread);
                assert!(calls > 0);
            }
            let (fresh, calls) =
                Statements::measure(thread_display_facts(&mut other, &[thread], None, later)).await;
            assert_eq!(fresh.unwrap()[0].id, thread);
            assert!(calls > 0);
        })
    })
    .await;
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn an_opening_reuses_the_owner_and_role_rows_of_each_requested_place() {
    let (world, _owner, _reader, root, project, _) = fixture(false).await;
    let at = world.now();
    let later = at + chrono::Duration::seconds(1);
    let mut tx = world.app.app.db.begin_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let pages = [
                PageKey::new("node", root, "live"),
                PageKey::new("node", project, "live"),
            ];
            let first = access::owner_entries(conn, &pages).await.unwrap();
            let (again, calls) = Statements::measure(access::owner_entries(
                conn,
                &[pages[1].clone(), pages[0].clone()],
            ))
            .await;
            assert_eq!(again.unwrap(), first);
            assert_eq!(calls, 0);
            let roles = access::roles_in_places(conn, &[root, project], at)
                .await
                .unwrap();
            let (again, calls) =
                Statements::measure(access::roles_in_places(conn, &[project, root], at)).await;
            assert_eq!(again.unwrap(), roles);
            assert_eq!(calls, 0);
            let (fresh, calls) =
                Statements::measure(access::roles_in_places(conn, &[root, project], later)).await;
            fresh.unwrap();
            assert!(calls > 0);
            let owners = access::owner_entries(conn, &pages[1..]).await.unwrap();
            assert_eq!(
                owners,
                first
                    .into_iter()
                    .filter(|row| row.object_id == project)
                    .collect::<Vec<_>>()
            );
            let subset = access::roles_in_places(conn, &[project], at).await.unwrap();
            assert_eq!(
                subset,
                roles
                    .into_iter()
                    .filter(|row| row.node_id == project)
                    .collect::<Vec<_>>()
            );
        })
    })
    .await;
    tx.rollback().await.unwrap();
}
