//! Initial-file creation uses the same creator operation's exact source reads. Raw HTTP replies
//! keep this control usable on the existing-Environment creator baseline before this branch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod creator_support;
mod places;
mod rig;
mod support;

use creator_support::{body, key, managed, rule, thread};
use orch_api_contract::binding::{EnvironmentChoice, NewEnvironment, NewEnvironmentFiles};
use orch_api_contract::forks::FileSelection;
use orch_db::queries::{jobs, threads};
use orch_types::{Id, Json};
use rig::Scene;
use serde_json::json;

async fn counts(s: &Scene) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM environments),(SELECT count(*) FROM threads),(SELECT count(*) FROM jobs)")
        .fetch_one(s.rig.db.pool()).await.unwrap()
}

#[tokio::test]
async fn a_new_environment_initial_copy_requires_the_creators_actual_source_reads() {
    let s = Scene::new().await;
    let source = s.thread().await;
    let (host, template) = managed(&s).await;
    let topic = s.tree.topic.base.id;
    let mut requested = body(&s);
    requested.environment = Some(EnvironmentChoice::New(Box::new(NewEnvironment {
        name: Some("Scoped initial copy".into()),
        home_node_id: Some(topic),
        template_id: Some(template),
        host_id: Some(Some(host)),
        files: Some(NewEnvironmentFiles::FromThread {
            from_thread: source.base.id,
            selection: Some(FileSelection::Selection {
                include: vec!["README.md".into()],
                exclude: None,
            }),
        }),
        ..Default::default()
    })));
    let mut rules = vec![
        rule(
            "ThreadCreate",
            "topic",
            topic,
            "self",
            &["thread"],
            &["write"],
        ),
        rule(
            "ThreadCreate",
            "topic",
            topic,
            "self",
            &["environment"],
            &["write"],
        ),
    ];
    let before = counts(&s).await;
    let token = key(&s, s.ada.id(), rules.clone()).await;
    let (status, reply) = thread(&s, &token, topic, &requested).await;
    assert_eq!(status, 403, "missing copy reads: {reply}");
    assert_eq!(
        counts(&s).await,
        before,
        "no Environment, Thread or job escapes the denied prefix"
    );

    rules.push(rule(
        "ThreadCreate",
        "thread",
        source.base.id,
        "self",
        &["thread"],
        &["read"],
    ));
    let token = key(&s, s.ada.id(), rules.clone()).await;
    let (status, reply) = thread(&s, &token, topic, &requested).await;
    assert_eq!(
        status, 403,
        "source Thread read alone does not read its Environment files: {reply}"
    );
    assert_eq!(counts(&s).await, before);

    rules.push(rule(
        "ThreadCreate",
        "environment",
        s.on.environment,
        "self",
        &["environment"],
        &["read"],
    ));
    let token = key(&s, s.ada.id(), rules).await;
    let (status, reply) = thread(&s, &token, topic, &requested).await;
    assert_eq!(status, 200, "complete same-operation source reads: {reply}");
    assert_eq!(reply["projection"], "receipt");
    assert_eq!(reply.as_object().unwrap().len(), 4);
    let created: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let mut conn = s.rig.world.conn().await;
    let stored = threads::thread(&mut conn, created).await.unwrap().unwrap();
    let target = stored.environment_id.unwrap();
    assert_ne!(target, s.on.environment);
    let job: Id = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE kind='environment_provision' AND environment_id=$1",
    )
    .bind(target)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let queued = jobs::job(&mut conn, job).await.unwrap().unwrap();
    let initial: &Json = &queued.plan["initial_files"];
    assert_eq!(initial["thread"], json!(source.base.id));
    assert_eq!(initial["environment"], json!(s.on.environment));
    assert_eq!(initial["creator_thread"], json!(created));
    assert_eq!(initial["target_environment"], json!(target));
    assert_eq!(initial["target_generation"], 0);
    assert_eq!(initial["creator_origin"]["operation"], "ThreadCreate");
    assert_eq!(
        initial["selection"],
        json!({"include":["README.md"],"exclude":[]})
    );
}

#[tokio::test]
async fn a_saved_blob_snapshot_uses_its_actual_thread_and_recorded_environment_read_authority() {
    use orch_db::queries::{environments::thread_engine as environments, storage};
    use orch_engine::manifest::{self, Entry, EntryKind, Header};
    use orch_types::{Time, new_id};

    let s = Scene::new().await;
    let source = s.thread().await;
    let (host, template) = managed(&s).await;
    let now = s.rig.world.now();
    let mut tx = s.rig.db.begin().await.unwrap();
    let source_thread = threads::thread(&mut tx, source.base.id)
        .await
        .unwrap()
        .unwrap();
    let source_environment = source_thread.environment_id.unwrap();
    let source_env = environments::environment(&mut tx, source_environment)
        .await
        .unwrap()
        .unwrap();
    let file = s
        .rig
        .world
        .app
        .app
        .blobs
        .put_bytes(bytes::Bytes::from_static(
            b"the exact saved initial README\n",
        ))
        .await
        .unwrap();
    let written = manifest::build(
        &Header {
            v: manifest::MANIFEST_VERSION,
            root: orch_engine::sessions::absolute_directory(
                &source_env.workspace_root,
                &source_thread.workspace_dir,
            ),
            generated_at: Time::new(now),
            generation: i64::from(source_env.generation),
            case_sensitive: true,
        },
        vec![Entry {
            path: "README.md".into(),
            kind: EntryKind::File,
            size: file.size,
            mtime_ns: 0,
            mode: manifest::DEFAULT_FILE_MODE,
            hash: Some(file.hash),
            link: None,
        }],
    )
    .unwrap();
    let list = s
        .rig
        .world
        .app
        .app
        .blobs
        .put_bytes(bytes::Bytes::from(written.bytes))
        .await
        .unwrap();
    storage::register_blob(&mut tx, &file.hash, file.size, now)
        .await
        .unwrap();
    storage::register_blob(&mut tx, &list.hash, list.size, now)
        .await
        .unwrap();
    let snapshot = new_id();
    storage::begin_snapshot(
        &mut tx,
        &storage::NewSnapshot {
            id: snapshot,
            thread_id: Some(source.base.id),
            environment_id: source_environment,
            env_generation: source_env.generation,
            label: "Complete saved initial files",
            selection: &json!({"include":[source_thread.workspace_dir],"exclude":[]}),
            created_by: Some(s.ada.id()),
            at: now,
        },
    )
    .await
    .unwrap();
    assert!(
        storage::end_snapshot(
            &mut tx,
            snapshot,
            &storage::SnapshotEnd {
                state: storage::snapshot_state::COMPLETE,
                file_count: 1,
                total_bytes: file.size,
                missing_count: 0,
                manifest_blob: Some(list.hash),
                at: now,
            }
        )
        .await
        .unwrap()
    );
    assert_eq!(
        storage::pin_snapshot(&mut tx, snapshot, &[list.hash, file.hash])
            .await
            .unwrap(),
        2
    );
    tx.commit().await.unwrap();

    let topic = s.tree.topic.base.id;
    let mut requested = body(&s);
    requested.environment = Some(EnvironmentChoice::New(Box::new(NewEnvironment {
        name: Some("Scoped saved snapshot target".into()),
        home_node_id: Some(topic),
        template_id: Some(template),
        host_id: Some(Some(host)),
        files: Some(NewEnvironmentFiles::Snapshot {
            snapshot_id: snapshot,
        }),
        ..Default::default()
    })));
    let mut rules = vec![
        rule(
            "ThreadCreate",
            "topic",
            topic,
            "self",
            &["thread"],
            &["write"],
        ),
        rule(
            "ThreadCreate",
            "topic",
            topic,
            "self",
            &["environment"],
            &["write"],
        ),
    ];
    let before = counts(&s).await;
    let token = key(&s, s.ada.id(), rules.clone()).await;
    let (status, reply) = thread(&s, &token, topic, &requested).await;
    assert_eq!(
        status, 403,
        "a known snapshot ID alone is no source authority: {reply}"
    );
    assert_eq!(counts(&s).await, before);
    rules.push(rule(
        "ThreadCreate",
        "thread",
        source.base.id,
        "self",
        &["thread"],
        &["read"],
    ));
    let token = key(&s, s.ada.id(), rules.clone()).await;
    let (status, reply) = thread(&s, &token, topic, &requested).await;
    assert_eq!(
        status, 403,
        "the saved source still requires recorded Environment read: {reply}"
    );
    assert_eq!(counts(&s).await, before);
    rules.push(rule(
        "ThreadCreate",
        "environment",
        source_environment,
        "self",
        &["environment"],
        &["read"],
    ));
    let token = key(&s, s.ada.id(), rules).await;
    let (status, reply) = thread(&s, &token, topic, &requested).await;
    s.rig.engine.stop();
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["projection"], "receipt");
    let created: Id = serde_json::from_value(reply["thread_id"].clone()).unwrap();
    let mut conn = s.rig.world.conn().await;
    let target = threads::thread(&mut conn, created)
        .await
        .unwrap()
        .unwrap()
        .environment_id
        .unwrap();
    let plan: Json = sqlx::query_scalar(
        "SELECT plan FROM jobs WHERE kind='environment_provision' AND environment_id=$1",
    )
    .bind(target)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let initial = &plan["initial_files"];
    assert_eq!(initial["snapshot"], json!(snapshot));
    assert_eq!(initial["manifest"], json!(list.hash));
    assert_eq!(initial["thread"], json!(source.base.id));
    assert_eq!(initial["environment"], json!(source_environment));
    assert_eq!(initial["generation"], source_env.generation);
    assert_eq!(initial["local"], false);
    assert_eq!(initial["creator_thread"], json!(created));
    assert_eq!(initial["target_environment"], json!(target));
    assert_eq!(initial["target_generation"], 0);
    assert_eq!(initial["creator_origin"]["operation"], "ThreadCreate");
    assert_eq!(
        storage::snapshot(&mut conn, snapshot)
            .await
            .unwrap()
            .unwrap()
            .manifest_blob,
        Some(list.hash),
        "admission never recaptures or substitutes the saved source"
    );
}
