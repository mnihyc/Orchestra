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
    let (world, _owner, reader, root, project, _) = fixture(false).await;
    let at = world.now();
    testing::add_entry(
        &mut *world.conn().await,
        &PageKey::new("node", project, "live"),
        "any",
        "access.own",
        ("user", Some(reader.id()), None),
        None,
        at,
    )
    .await
    .unwrap();
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
            let (owners, calls) = Statements::measure(access::owner_entries(
                conn,
                &[pages[1].clone(), pages[1].clone()],
            ))
            .await;
            let expected: Vec<_> = first
                .into_iter()
                .filter(|row| row.object_id == project)
                .flat_map(|row| [row.clone(), row])
                .collect();
            assert!(!expected.is_empty());
            assert_eq!(owners.unwrap(), expected);
            assert_eq!(
                calls, 0,
                "a repeated selected object keeps its owners and their multiplicity"
            );
            let (subset, calls) =
                Statements::measure(access::roles_in_places(conn, &[project, project], at)).await;
            assert_eq!(calls, 0, "a subset keeps its roles at the original clock");
            let subset = subset.unwrap();
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

#[tokio::test]
async fn a_signed_in_reader_can_immediately_reuse_the_connection_that_checked_the_session() {
    use orch_api_core::middleware::authentication::{self, Caller as HttpCaller};
    let world = World::for_requests().await;
    let reader = person(&world, "opening-reader").await;
    let app = &world.app.app;
    let pool = app.db.pool();
    let mut reserved = Vec::new();
    for _ in 0..pool.options().get_max_connections() {
        reserved.push(pool.acquire().await.unwrap());
    }
    reserved.pop().unwrap().return_to_pool().await;
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {}", reader.token).parse().unwrap(),
    );
    let identity = authentication::authenticate(app, &headers, world.now())
        .await
        .unwrap();
    assert!(matches!(identity, HttpCaller::User(_)));
    assert_eq!(
        pool.num_idle(),
        1,
        "the opening can take the established connection as soon as authentication returns"
    );
}

#[tokio::test]
async fn an_opening_reads_each_path_resource_and_thread_once_and_a_writer_still_records_authority()
{
    use orch_db::queries::{hierarchy, resources, threads, transaction_guard};

    let (world, _, [org, _, topic, thread, _]) = super::page_budgets::fixture(1, true).await;
    let db = &world.app.app.db;
    let (provider, environment): (Id, Id) =
        sqlx::query_as("SELECT provider_id, environment_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(db.pool())
            .await
            .unwrap();
    let mut tx = db.begin_opening_snapshot().await.unwrap();
    let read_only: bool =
        sqlx::query_scalar("SELECT current_setting('transaction_read_only')::boolean")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(read_only);
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let mut nodes = hierarchy::with_ancestors(conn, &[thread, topic])
                .await
                .unwrap();
            nodes.sort_by_key(|row| row.id);
            assert!(nodes.iter().any(|row| row.id == thread));
            let (again, calls) =
                Statements::measure(hierarchy::with_ancestor_governance(conn, &[topic, thread]))
                    .await;
            let mut again: Vec<_> = again.unwrap().into_iter().map(|(row, _)| row).collect();
            again.sort_by_key(|row| row.id);
            assert_eq!(again, nodes);
            assert_eq!(calls, 0);
            let facts = resources::resource_facts(conn, &[provider, environment])
                .await
                .unwrap();
            assert!(facts.iter().any(|row| row.id == environment));
            let (again, calls) =
                Statements::measure(resources::resource_facts(conn, &[environment, provider]))
                    .await;
            assert_eq!(again.unwrap(), facts);
            assert_eq!(calls, 0);
            let row = threads::thread(conn, thread).await.unwrap().unwrap();
            let (again, calls) = Statements::measure(threads::threads_among(conn, &[thread])).await;
            assert_eq!(again.unwrap(), [row]);
            assert_eq!(calls, 0);
        })
    })
    .await;
    tx.commit().await.unwrap();

    let mut guarded = transaction_guard::request_scope(
        transaction_guard::Scope::Shared,
        db.begin_opening_snapshot(),
    )
    .await
    .unwrap();
    let writable: bool = sqlx::query_scalar("SELECT NOT current_setting('transaction_read_only')::boolean AND current_setting('orch.authz_seen')::jsonb <> '{}'::jsonb")
        .fetch_one(&mut *guarded).await.unwrap();
    assert!(
        writable,
        "the enclosing transaction can record its authority dependencies"
    );
    guarded.rollback().await.unwrap();

    for use_resource in [false, true] {
        let mut tx = db.begin_snapshot().await.unwrap();
        read_facts::scope(&mut tx, move |conn| {
            Box::pin(async move {
                if use_resource {
                    resources::resource_facts(conn, &[environment])
                        .await
                        .unwrap();
                } else {
                    hierarchy::with_ancestors(conn, &[thread]).await.unwrap();
                }
                transaction_guard::lock_action(conn).await.unwrap();
                if use_resource {
                    resources::resource_facts(conn, &[environment])
                        .await
                        .unwrap();
                } else {
                    hierarchy::with_ancestors(conn, &[thread]).await.unwrap();
                }
                let watched: bool =
                    sqlx::query_scalar("SELECT current_setting('orch.authz_seen')::jsonb ? $1")
                        .bind(org.to_string())
                        .fetch_one(conn)
                        .await
                        .unwrap();
                assert!(
                    watched,
                    "a dependency read after arming a write must record its root"
                );
            })
        })
        .await;
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
async fn an_opening_reads_chosen_settings_once_and_leaves_other_places_and_values_independent() {
    use orch_db::queries::settings::rows_on_chain;
    let (world, _, [org, project, topic, thread, _]) = super::page_budgets::fixture(1, true).await;
    let db = &world.app.app.db;
    let mut tx = db.begin_opening_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let path = [org, project, topic, thread];
            let model = ["agent.model".to_owned()];
            let rows = rows_on_chain(conn, &path, Some(&model)).await.unwrap();
            assert!(
                rows.iter()
                    .any(|row| row.node_id == Some(thread) && row.value == "scale-model")
            );
            let absent = ["agent.effort".to_owned()];
            let (defaults, calls) =
                Statements::measure(rows_on_chain(conn, &path, Some(&absent))).await;
            let all = rows_on_chain(conn, &path, None).await.unwrap();
            assert_eq!(
                defaults.unwrap().len(),
                all.iter().filter(|row| row.key == absent[0]).count()
            );
            assert_eq!(
                calls, 0,
                "a default on this path needs no additional database read"
            );
            let (subset, calls) =
                Statements::measure(rows_on_chain(conn, &[topic, org], Some(&model))).await;
            assert!(
                subset
                    .unwrap()
                    .iter()
                    .all(|row| row.node_id.is_none_or(|id| id == topic || id == org))
            );
            assert_eq!(calls, 0);
            let (again, calls) = Statements::measure(rows_on_chain(
                conn,
                &[thread, topic, project, org, thread],
                Some(&model),
            ))
            .await;
            assert_eq!(again.unwrap(), rows);
            assert_eq!(calls, 0);
        })
    })
    .await;
    tx.commit().await.unwrap();
    sqlx::query("UPDATE settings SET value = to_jsonb('changed-model'::text) WHERE key='agent.model' AND node_id=$1")
        .bind(thread).execute(db.pool()).await.unwrap();
    let mut fresh = db.begin_opening_snapshot().await.unwrap();
    read_facts::scope(&mut fresh, move |conn| {
        Box::pin(async move {
            let rows = rows_on_chain(conn, &[thread], Some(&["agent.model".to_owned()]))
                .await
                .unwrap();
            assert!(
                rows.iter()
                    .any(|row| row.node_id == Some(thread) && row.value == "changed-model")
            );
        })
    })
    .await;
    fresh.commit().await.unwrap();
}

#[tokio::test]
async fn the_browser_opening_releases_its_sampled_session_connection_before_reading_the_thread() {
    use orch_api_core::testing::{Boundaries, follow};
    let (world, reader, [_, _, _, thread, _]) = super::page_budgets::fixture(1, true).await;
    let pool = world.app.app.db.pool().clone();
    let mut reserved = Vec::new();
    for _ in 0..pool.options().get_max_connections() {
        reserved.push(pool.acquire().await.unwrap());
    }
    reserved.pop().unwrap().return_to_pool().await;
    let remaining = std::sync::Mutex::new(Some(reserved));
    let trace = Boundaries::new();
    trace.hold(move |op, phase| {
        if op == "ThreadOpeningGet"
            && phase == "opening.identity_sampled"
            && let Some(held) = remaining.lock().unwrap().take()
        {
            assert_eq!(
                pool.num_idle(),
                1,
                "the freshly checked connection is ready for the projection"
            );
            drop(held);
        }
        Box::pin(async {})
    });
    let reply = follow(
        trace.clone(),
        world
            .app
            .call(orch_api_contract::threads::ThreadOpeningGet, &[&thread])
            .token(&reader.token)
            .send(),
    )
    .await;
    assert!(!reply.ok().sync.items.is_empty());
    assert!(trace.entered("ThreadOpeningGet", "opening.identity_sampled"));
}

#[tokio::test]
async fn opening_a_directory_shows_its_report_and_uses_its_current_permission_defaults() {
    use orch_access_contract::permissions::Placement;
    use orch_db::queries::environment_facts::{access_profiles, of};

    let (world, _, [_, _, _, thread, _]) = super::page_budgets::fixture(1, true).await;
    let db = &world.app.app.db;
    let (environment, machine): (Id, Id) = sqlx::query_as(
        "SELECT e.id, e.machine_id FROM threads t JOIN environments e ON e.id=t.environment_id WHERE t.node_id=$1",
    ).bind(thread).fetch_one(db.pool()).await.unwrap();

    for (delegated, expected) in [
        ("environment.roots", Placement::Connected),
        ("environment.*", Placement::Managed),
    ] {
        let report = serde_json::json!({
            "environment_descriptor": {"lifecycle": "attach", "confinement": "none"},
            "policy": {"policy.delegated": [delegated]}
        });
        sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1")
            .bind(machine)
            .bind(report)
            .execute(db.pool())
            .await
            .unwrap();
        let mut independent = world.conn().await;
        let native = of(&mut independent, environment).await.unwrap().unwrap();
        assert_eq!(native.machine_id, Some(machine));
        assert!(native.attaches());
        drop(independent);
        let mut tx = db.begin_opening_snapshot().await.unwrap();
        read_facts::scope(&mut tx, move |conn| {
            Box::pin(async move {
                assert_eq!(
                    access_profiles(conn, &[environment]).await.unwrap(),
                    [(environment, expected)]
                );
                let (shown, calls) = Statements::measure(of(conn, environment)).await;
                assert_eq!(shown.unwrap(), Some(native));
                assert_eq!(calls, 0, "the opening already read the registered machine");
            })
        })
        .await;
        tx.commit().await.unwrap();
    }
}

#[tokio::test]
async fn an_opening_reads_each_named_person_once_across_overlapping_batches() {
    use orch_db::queries::{identity, threads};
    let (world, owner, reader, _, project, _) = fixture(false).await;
    let owner = owner.id();
    let reader = reader.id();
    let mut tx = world.app.app.db.begin_opening_snapshot().await.unwrap();
    read_facts::scope(&mut tx, move |conn| {
        Box::pin(async move {
            let (first, calls) =
                Statements::measure(threads::people(conn, &[owner, owner, project])).await;
            let first = first.unwrap();
            assert_eq!(calls, 1);
            assert_eq!(first.len(), 1);
            assert_eq!(first[0].id, owner);

            let (mixed, calls) =
                Statements::measure(identity::users_brief(conn, &[reader, owner])).await;
            assert_eq!(calls, 1);
            let mut mixed = mixed.unwrap();
            mixed.sort_by_key(|row| row.id);
            assert_eq!(mixed.len(), 2);
            assert!(mixed.iter().any(|row| row.id == reader));
            assert!(mixed.iter().any(|row| row.id == owner));

            let (again, calls) =
                Statements::measure(threads::people(conn, &[project, reader, owner, reader])).await;
            assert_eq!(calls, 0);
            let mut again = again.unwrap();
            again.sort_by_key(|row| row.id);
            assert_eq!(
                again.iter().map(|row| row.id).collect::<Vec<_>>(),
                mixed.iter().map(|row| row.id).collect::<Vec<_>>()
            );
        })
    })
    .await;
    tx.commit().await.unwrap();
}
