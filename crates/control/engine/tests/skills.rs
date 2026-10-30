//! Skills reach a thread's agent as its places switch them on, at their latest or a pinned version, while whoever named them may use them; a person's own skills follow them into the threads they start where the place allows; versions keep their name, pin their archive and are recorded on the turns that had them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;

use common::World;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::skills::{SkillOrigin, SkillSkip, ThreadSkillFrom, ThreadSkills};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_blob_contract::store::BlobStore;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::access::{PageKey, testing as access_testing};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_engine::skills::archive::{File, Limits};
use orch_engine::skills::resolve;
use orch_engine::skills::versions::{self, Made, SkillError, Target};
use orch_settings::{Target as SettingTarget, write as settings_write};
use orch_types::{Id, Time};
use serde_json::{Value, json};

const LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024,
    max_files: 100,
};

/// A store of blobs in a directory of its own, kept for as long as the test runs.
struct Store {
    store: Arc<dyn BlobStore>,
    _dir: tempfile::TempDir,
}

async fn store() -> Store {
    let dir = tempfile::tempdir().unwrap();
    let store = orch_blob_local::LocalBlobStore::open(dir.path())
        .await
        .unwrap();
    Store {
        store: Arc::new(store),
        _dir: dir,
    }
}

fn files(name: &str, body: &str) -> Vec<File> {
    vec![
        File {
            path: "SKILL.md".to_owned(),
            data: format!(
                "---\nname: {name}\ndescription: Does {name} things. Use it when asked.\n---\n{body}\n"
            )
            .into_bytes(),
            executable: false,
        },
        File {
            path: "scripts/run.sh".to_owned(),
            data: b"#!/bin/sh\necho run\n".to_vec(),
            executable: true,
        },
    ]
}

/// Makes a version of a new skill, or of the skill `skill`, from `files`, as `by`.
async fn version(
    world: &World,
    store: &Store,
    target: Target,
    files: Vec<File>,
    by: Id,
) -> Result<Made, SkillError> {
    let prepared = versions::prepare_files(&store.store, files, LIMITS).await?;
    let mut tx = world.pool.begin().await.unwrap();
    let made = versions::record(
        &mut tx,
        target,
        &prepared,
        &SkillOrigin::Draft { based_on: None },
        Some(by),
        Time::new(world.clock.now()),
    )
    .await?;
    if let Target::NewSkill { id, owner, .. } = target {
        start_resource(
            &mut tx,
            ObjectRef {
                kind: ResourceKind::Skill.into(),
                id,
            },
            owner,
            NewAccess::OnlyMe,
            Time::new(world.clock.now()),
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    Ok(made)
}

/// A new skill named `name` at `home`, first owned by `owner`, with one version.
async fn skill(world: &World, store: &Store, home: Id, owner: Id, name: &str) -> Made {
    version(
        world,
        store,
        Target::NewSkill {
            id: orch_types::new_id(),
            home,
            owner,
        },
        files(name, "first"),
        owner,
    )
    .await
    .unwrap()
}

/// Writes `agents.skills` (or another key) at a place as `writer`, locked or not.
async fn set_as(world: &World, writer: Id, node: Id, key: &str, value: Value, locked: bool) {
    let mut conn = world.pool.acquire().await.unwrap();
    settings_write::write(
        &mut conn,
        &settings_write::Write {
            actor: Actor::User(writer),
            target: SettingTarget::Node(node),
            key,
            value,
            locked: Some(locked),
            expected_revision: None,
            may_lock: true,
            origin: &Origin::default(),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
}

async fn given(world: &World, thread: Id) -> ThreadSkills {
    let mut conn = world.pool.acquire().await.unwrap();
    resolve::for_thread(&mut conn, thread, Time::new(world.clock.now()))
        .await
        .unwrap()
}

fn names(skills: &ThreadSkills) -> Vec<(&str, i64)> {
    skills
        .skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill.number))
        .collect()
}

fn reasons(skills: &ThreadSkills) -> Vec<SkillSkip> {
    skills
        .skipped
        .iter()
        .map(|skipped| skipped.reason)
        .collect()
}

#[tokio::test]
async fn a_skill_switched_on_at_a_place_reaches_its_threads_at_its_latest_or_a_pinned_version() {
    let world = World::new().await;
    let store = store().await;
    let first = skill(&world, &store, world.root, world.ada, "pdf-tools").await;
    let id = first.skill.id;
    world
        .set(
            world.root,
            "agents.skills",
            json!({ id.to_string(): "latest" }),
        )
        .await;
    let thread = world.thread("Reports").await;
    let skills = given(&world, thread).await;
    assert_eq!(names(&skills), [("pdf-tools", 1)]);
    let one = &skills.skills[0];
    assert_eq!(one.version_id, first.version.id);
    assert_eq!(
        one.from,
        ThreadSkillFrom::Place {
            node_id: world.root
        }
    );
    let paths: Vec<&str> = one.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["SKILL.md", "scripts/run.sh"]);
    assert!(one.files[1].executable);
    assert!(store.store.exists(&one.archive.blob).await.unwrap());

    let second = version(
        &world,
        &store,
        Target::Skill(id),
        files("pdf-tools", "second"),
        world.ada,
    )
    .await
    .unwrap();
    assert_eq!(second.version.number, 2);
    assert_eq!(
        names(&given(&world, thread).await),
        [("pdf-tools", 2)],
        "latest follows each new version"
    );

    world
        .set(
            world.topic,
            "agents.skills",
            json!({ id.to_string(): first.version.id.to_string() }),
        )
        .await;
    assert_eq!(
        names(&given(&world, thread).await),
        [("pdf-tools", 1)],
        "a place below pins the version its threads get"
    );
    world
        .set(
            world.topic,
            "agents.skills",
            json!({ id.to_string(): "off" }),
        )
        .await;
    let off = given(&world, thread).await;
    assert!(off.skills.is_empty() && off.skipped.is_empty(), "{off:?}");
}

#[tokio::test]
async fn a_lock_above_keeps_what_the_places_below_say_out() {
    let world = World::new().await;
    let store = store().await;
    let made = skill(&world, &store, world.root, world.ada, "locked-in").await;
    let id = made.skill.id.to_string();
    // The row below is written first: once the lock is there, nobody may write under it.
    world
        .set(world.topic, "agents.skills", json!({ &id: "off" }))
        .await;
    set_as(
        &world,
        world.ada,
        world.root,
        "agents.skills",
        json!({ &id: "latest" }),
        true,
    )
    .await;
    let thread = world.thread("Locked").await;
    assert_eq!(names(&given(&world, thread).await), [("locked-in", 1)]);
}

#[tokio::test]
async fn whoever_switched_a_skill_on_must_be_able_to_use_it_when_a_thread_asks() {
    let world = World::new().await;
    let store = store().await;
    let owner = world.person("skill-owner").await;
    let made = skill(&world, &store, world.root, owner, "team-skill").await;
    let id = made.skill.id;
    world
        .set(
            world.root,
            "agents.skills",
            json!({ id.to_string(): "latest" }),
        )
        .await;
    let thread = world.thread("Work").await;
    let refused = given(&world, thread).await;
    assert!(refused.skills.is_empty());
    assert_eq!(reasons(&refused), [SkillSkip::NotUsable]);

    // Its use given to the work in the project lets every thread there have it.
    let mut conn = world.pool.acquire().await.unwrap();
    let grant = access_testing::add_entry(
        &mut conn,
        &PageKey::new("skill", id, "live"),
        "any",
        "skill.use",
        ("node", Some(world.project), None),
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(names(&given(&world, thread).await), [("team-skill", 1)]);

    // Taking it back stops it at the next ask, without the setting being touched.
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_db::queries::access::remove_entry(&mut conn, grant, owner, world.clock.now())
            .await
            .unwrap()
    );
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        reasons(&given(&world, thread).await),
        [SkillSkip::NotUsable]
    );
}

#[tokio::test]
async fn a_skills_use_given_to_one_thread_reaches_that_thread_alone() {
    let world = World::new().await;
    let store = store().await;
    let owner = world.person("skill-owner").await;
    let made = skill(&world, &store, world.root, owner, "only-here").await;
    let id = made.skill.id;
    world
        .set(
            world.root,
            "agents.skills",
            json!({ id.to_string(): "latest" }),
        )
        .await;
    let chosen = world.thread("Chosen").await;
    let other = world.thread("Other").await;
    let mut conn = world.pool.acquire().await.unwrap();
    access_testing::add_entry(
        &mut conn,
        &PageKey::new("skill", id, "live"),
        "any",
        "skill.use",
        ("node", Some(chosen), None),
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(names(&given(&world, chosen).await), [("only-here", 1)]);
    assert_eq!(reasons(&given(&world, other).await), [SkillSkip::NotUsable]);
}

/// Gives `person` an own org of their own, as a new account has.
async fn own_org(world: &World, person: Id) -> Id {
    let org = orch_types::new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    insert_node(&mut conn, org, "org", None, "Own", person)
        .await
        .unwrap();
    sqlx::query("UPDATE orgs SET org_kind='personal' WHERE node_id=$1")
        .bind(org)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET personal_org_id=$2 WHERE id=$1")
        .bind(person)
        .bind(org)
        .execute(&mut *conn)
        .await
        .unwrap();
    access_testing::insert_member(&mut conn, org, person, org)
        .await
        .unwrap();
    org
}

#[tokio::test]
async fn a_persons_own_skills_reach_the_threads_they_started_where_the_place_allows_them() {
    let world = World::new().await;
    let store = store().await;
    let own = own_org(&world, world.ada).await;
    let mine = skill(&world, &store, own, world.ada, "my-habits").await;
    let started = world.thread("Mine").await;
    let skills = given(&world, started).await;
    assert_eq!(names(&skills), [("my-habits", 1)]);
    assert_eq!(
        skills.skills[0].from,
        ThreadSkillFrom::Personal { user_id: world.ada }
    );

    // A thread somebody else started is not given them.
    let bea = world.person("bea").await;
    world.give(bea, "developer").await;
    let theirs = world.thread_by(bea, "Theirs").await;
    assert!(given(&world, theirs).await.skills.is_empty());

    // The person switches one of their own off where they keep them.
    world
        .set(
            own,
            "agents.skills",
            json!({ mine.skill.id.to_string(): "off" }),
        )
        .await;
    assert!(given(&world, started).await.skills.is_empty());
    world.unset(own, "agents.skills").await;
    assert_eq!(names(&given(&world, started).await), [("my-habits", 1)]);

    // A place that does not allow them keeps them out.
    world
        .set(world.project, "agents.personal_skills", json!(false))
        .await;
    assert!(given(&world, started).await.skills.is_empty());
}

#[tokio::test]
async fn a_disabled_or_deleted_skill_or_a_version_of_another_is_left_out_with_why() {
    let world = World::new().await;
    let store = store().await;
    let off = skill(&world, &store, world.root, world.ada, "switched-off").await;
    let gone = skill(&world, &store, world.root, world.ada, "gone").await;
    let pinned = skill(&world, &store, world.root, world.ada, "pinned").await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::resources::skills::set_enabled(
        &mut conn,
        off.skill.id,
        false,
        world.clock.now(),
    )
    .await
    .unwrap();
    orch_db::queries::resources::skills::delete(&mut conn, gone.skill.id, world.clock.now())
        .await
        .unwrap();
    drop(conn);
    world
        .set(
            world.root,
            "agents.skills",
            json!({
                off.skill.id.to_string(): "latest",
                gone.skill.id.to_string(): "latest",
                // A version of another skill is no version of this one.
                pinned.skill.id.to_string(): off.version.id.to_string(),
                orch_types::new_id().to_string(): "latest",
            }),
        )
        .await;
    let thread = world.thread("Odd").await;
    let skills = given(&world, thread).await;
    assert!(skills.skills.is_empty());
    let mut found = reasons(&skills);
    found.sort_by_key(|reason| format!("{reason:?}"));
    assert_eq!(
        found,
        [
            SkillSkip::Deleted,
            SkillSkip::Disabled,
            SkillSkip::Missing,
            SkillSkip::VersionNotFound
        ]
    );
}

#[tokio::test]
async fn two_skills_of_one_name_never_reach_one_agent_and_the_nearer_place_wins() {
    let world = World::new().await;
    let store = store().await;
    let upper = skill(&world, &store, world.root, world.ada, "same-name").await;
    let nearer = skill(&world, &store, world.project, world.ada, "same-name").await;
    world
        .set(
            world.root,
            "agents.skills",
            json!({ upper.skill.id.to_string(): "latest" }),
        )
        .await;
    world
        .set(
            world.topic,
            "agents.skills",
            json!({ nearer.skill.id.to_string(): "latest" }),
        )
        .await;
    let thread = world.thread("Clash").await;
    let skills = given(&world, thread).await;
    assert_eq!(skills.skills.len(), 1);
    assert_eq!(skills.skills[0].skill_id, nearer.skill.id);
    assert_eq!(reasons(&skills), [SkillSkip::NameTaken]);
}

#[tokio::test]
async fn a_version_keeps_its_skills_name_and_a_home_has_one_skill_of_a_name() {
    let world = World::new().await;
    let store = store().await;
    let made = skill(&world, &store, world.root, world.ada, "steady").await;
    let renamed = version(
        &world,
        &store,
        Target::Skill(made.skill.id),
        files("renamed", "x"),
        world.ada,
    )
    .await;
    assert!(
        matches!(renamed, Err(SkillError::NameChanged { ref skill, ref found }) if skill == "steady" && found == "renamed"),
        "{renamed:?}"
    );
    let twin = version(
        &world,
        &store,
        Target::NewSkill {
            id: orch_types::new_id(),
            home: world.root,
            owner: world.ada,
        },
        files("steady", "again"),
        world.ada,
    )
    .await;
    assert!(
        matches!(twin, Err(SkillError::NameTaken { .. })),
        "{twin:?}"
    );
    // Another home may have one of that name.
    let elsewhere = skill(&world, &store, world.project, world.ada, "steady").await;
    assert_eq!(elsewhere.version.number, 1);
}

#[tokio::test]
async fn a_version_pins_its_archive_and_the_same_files_make_the_same_archive() {
    let world = World::new().await;
    let store = store().await;
    let made = skill(&world, &store, world.root, world.ada, "pinned-down").await;
    let again = version(
        &world,
        &store,
        Target::Skill(made.skill.id),
        files("pinned-down", "first"),
        world.ada,
    )
    .await
    .unwrap();
    assert_eq!(
        made.version.archive, again.version.archive,
        "the same tree is the same archive"
    );
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(&made.version.archive)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(pins, 2, "each version pins the archive it names");
    let manifest = made.version.files.as_array().unwrap();
    assert_eq!(manifest.len(), 2);
    assert_eq!(manifest[1]["path"], "scripts/run.sh");
    assert_eq!(manifest[1]["executable"], true);
    use sha2::Digest;
    assert_eq!(
        manifest[1]["sha256"],
        hex::encode(sha2::Sha256::digest(b"#!/bin/sh\necho run\n"))
    );
}

#[tokio::test]
async fn a_turn_records_the_skill_versions_it_was_given() {
    let world = World::new().await;
    let store = store().await;
    let made = skill(&world, &store, world.root, world.ada, "recorded").await;
    world
        .set(
            world.root,
            "agents.skills",
            json!({ made.skill.id.to_string(): "latest" }),
        )
        .await;
    let thread = world.thread("Recorded").await;
    world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            thread,
            common::queued("Go"),
        )
        .await
        .unwrap();
    world.run_turn(thread, 0, "Done").await;
    let turn: Id = world
        .scalar(
            "SELECT id FROM turns WHERE thread_id=$1 ORDER BY ordinal LIMIT 1",
            thread,
        )
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::resources::skills::turn_versions(&mut conn, turn)
            .await
            .unwrap(),
        Some(vec![made.version.id]),
        "the turn records the versions its session was given"
    );
    drop(conn);
    let skills = world.engine().skills_for_thread(thread).await.unwrap();
    assert!(
        world
            .engine()
            .record_turn_skills(turn, &skills)
            .await
            .unwrap()
    );
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::resources::skills::turn_versions(&mut conn, turn)
            .await
            .unwrap(),
        Some(vec![made.version.id])
    );
    let held =
        orch_engine::membership::holdings(&mut conn, world.root, world.ada, world.clock.now())
            .await
            .unwrap();
    let threads = held
        .resources
        .iter()
        .find(|row| row.object.id == made.skill.id)
        .map(|row| row.threads);
    assert_eq!(
        threads,
        Some(1),
        "ending the membership names the thread it was given to"
    );
}

/// What the sessions of `thread` were opened with, oldest first.
fn opens(world: &World, thread: Id) -> Vec<orch_link_contract::commands::SessionOpen> {
    world
        .seen
        .of_kind(orch_link_contract::commands::CommandKind::SessionOpen)
        .into_iter()
        .filter_map(|command| match command {
            orch_link_contract::commands::Command::SessionOpen(open)
                if open.thread_id == thread =>
            {
                Some(open)
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_session_is_given_its_threads_skills_opened_anew_when_they_change_and_each_turn_records_them()
 {
    let world = World::new().await;
    let store = store().await;
    let made = skill(&world, &store, world.root, world.ada, "pdf-tools").await;
    world
        .set(
            world.root,
            "agents.skills",
            json!({ made.skill.id.to_string(): "latest" }),
        )
        .await;
    let thread = world.thread("Reports").await;
    world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            thread,
            common::queued("Start"),
        )
        .await
        .unwrap();
    let first = world.run_turn(thread, 0, "Fine").await;
    let opened = opens(&world, thread);
    let given: Vec<_> = opened
        .last()
        .unwrap()
        .skills
        .iter()
        .map(|skill| (skill.name.clone(), skill.version_id))
        .collect();
    assert_eq!(given, [("pdf-tools".to_owned(), made.version.id)]);
    let archive = &opened.last().unwrap().skills[0];
    assert!(store.store.exists(&archive.blob).await.unwrap());

    // A new version reaches the next turn: its session is opened anew with it before the turn.
    let second = version(
        &world,
        &store,
        Target::Skill(made.skill.id),
        files("pdf-tools", "second"),
        world.ada,
    )
    .await
    .unwrap();
    world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            thread,
            common::queued("Again"),
        )
        .await
        .unwrap();
    let next = world.run_turn(thread, 1, "Fine").await;
    let reopened = opens(&world, thread);
    assert!(reopened.len() > opened.len(), "the session was opened anew");
    assert_eq!(
        reopened.last().unwrap().skills[0].version_id,
        second.version.id
    );

    // Each turn records what it was given.
    for (turn, version) in [
        (first.turn_id, made.version.id),
        (next.turn_id, second.version.id),
    ] {
        let recorded: Option<Vec<Id>> =
            sqlx::query_scalar("SELECT skill_versions FROM turns WHERE id = $1")
                .bind(turn)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert_eq!(recorded, Some(vec![version]));
    }
}
