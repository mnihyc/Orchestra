//! Real keys restrict schedule authoring, disclosure and each actual future occurrence.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a control fails by panicking"
)]

mod places;
mod rig;
mod support;

use chrono::Duration;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::input_schedules::*;
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding};
use orch_api_contract::threads::{ContentPart, ThreadQueueResume, ThreadQueueResumeBody};
use orch_api_core::testing::RawReply;
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, Time, new_id};
use rig::Scene;
use serde_json::{Value, json};

const CREATE: &str = "TopicInputScheduleCreate";
const EDIT: &str = "InputScheduleUpdate.definition";
const STATE: &str = "InputScheduleUpdate.state";
const SECRET: &str = "private prepared schedule words";

struct Key {
    id: Id,
    secret: String,
}

#[derive(Debug, Clone, Copy)]
struct Receipt {
    id: Id,
    revision: i64,
}

fn rule(operation: &str, kind: &str, id: Id, resources: &[&str], actions: &[&str]) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":resources,
        "allow":[{"operation":operation,"actions":actions}]})
}

fn creation_rules(topic: Id, destination_kind: &str, destination: Id) -> Vec<Value> {
    vec![
        rule(CREATE, "topic", topic, &["schedule"], &["manage"]),
        rule(
            CREATE,
            destination_kind,
            destination,
            &["thread"],
            &["write"],
        ),
    ]
}

fn editing_rules(schedule: Id, destination: Id) -> Vec<Value> {
    vec![
        rule(EDIT, "schedule", schedule, &["schedule"], &["manage"]),
        rule(EDIT, "thread", destination, &["thread"], &["write"]),
    ]
}

async fn key(s: &Scene, owner: Id, rules: Vec<Value>) -> Key {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = s.rig.world.now();
    create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id,
            user_id: owner,
            name: "Scheduled operation control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            // The fixed-clock due claims below need minutes, not an unbounded credential.
            expires_at: Some(now + Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn revoke(s: &Scene, key: &Key) {
    let changed =
        sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1 AND revoked_at IS NULL")
            .bind(key.id)
            .bind(s.rig.world.now())
            .execute(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(changed.rows_affected(), 1);
}

async fn policy(s: &Scene, key: &Key, rules: Vec<Value>) {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let changed = sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(serde_json::to_value(scope).unwrap())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(changed.rows_affected(), 1);
}

fn once(s: &Scene, target: InputScheduleTarget, words: &str) -> InputScheduleCreateBody {
    InputScheduleCreateBody {
        name: Some("Private schedule label".into()),
        target,
        content: vec![ContentPart::Text { text: words.into() }],
        native_action: None,
        timing: InputTiming::Once {
            at: Time::new(s.rig.world.now() + Duration::minutes(1)),
            zone: "UTC".into(),
        },
    }
}

fn to_thread(id: Id) -> InputScheduleTarget {
    InputScheduleTarget::Thread { thread_id: id }
}

async fn create(s: &Scene, token: &str, body: &InputScheduleCreateBody) -> RawReply {
    s.rig
        .world
        .app
        .call(TopicInputScheduleCreate, &[&s.tree.topic.base.id])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}

async fn update(s: &Scene, token: &str, id: Id, body: &InputScheduleUpdateBody) -> RawReply {
    s.rig
        .world
        .app
        .call(InputScheduleUpdate, &[&id])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}

fn receipt(reply: RawReply, status: u16) -> Receipt {
    assert_eq!(
        reply.status.as_u16(),
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    let id: Id = serde_json::from_value(raw["schedule_id"].clone()).unwrap();
    let revision = raw["revision"].as_i64().unwrap();
    let topic_seq = raw["topic_seq"].as_i64().unwrap();
    assert_eq!(
        raw,
        json!({"projection":"receipt","schedule_id":id,"revision":revision,"topic_seq":topic_seq}),
        "an acknowledgement contains no stored draft, names, timing or related resource projection"
    );
    Receipt { id, revision }
}

fn full(reply: RawReply, status: u16) -> InputSchedule {
    assert_eq!(
        reply.status.as_u16(),
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert!(raw.get("projection").is_none());
    serde_json::from_value(raw).unwrap()
}

async fn get(s: &Scene, id: Id) -> InputSchedule {
    s.rig
        .world
        .app
        .call(InputScheduleGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
}

async fn hold(s: &Scene, thread: Id) {
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(thread)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

async fn release(s: &Scene, thread: Id) {
    let row = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, thread)
        .await
        .unwrap()
        .unwrap();
    s.rig
        .world
        .app
        .call(ThreadQueueResume, &[&thread])
        .token(&s.ada.token)
        .body(&ThreadQueueResumeBody {
            revision: row.queue_revision,
        })
        .send()
        .await
        .ok();
    s.rig.engine.preflight(thread).await.unwrap();
}

async fn due(s: &Scene) -> u64 {
    orch_api_core::input_schedules::run_due(&s.rig.world.app.app)
        .await
        .unwrap()
}

async fn input_count(s: &Scene, thread: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

async fn input_authority(s: &Scene, input: Id) -> Value {
    sqlx::query_scalar("SELECT submission_authority FROM thread_inputs WHERE id=$1")
        .bind(input)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

async fn schedule_state(s: &Scene, id: Id) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('name',name,'content',content,'target',target,'state',state,'revision',revision) FROM input_schedules WHERE id=$1")
        .bind(id).fetch_one(s.rig.db.pool()).await.unwrap()
}

fn denied(reply: RawReply) {
    assert_eq!(
        reply.status.as_u16(),
        403,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn scoped_and_browser_schedules_run_once_through_the_same_actual_machine_path() {
    let s = Scene::new().await;
    let keyed_thread = s.thread_by(&s.ada, "Key scheduled thread").await.base.id;
    let browser_thread = s
        .thread_by(&s.ada, "Browser scheduled thread")
        .await
        .base
        .id;
    hold(&s, keyed_thread).await;
    hold(&s, browser_thread).await;
    let author = key(
        &s,
        s.ada.id(),
        creation_rules(s.tree.topic.base.id, "thread", keyed_thread),
    )
    .await;
    let keyed = receipt(
        create(
            &s,
            &author.secret,
            &once(&s, to_thread(keyed_thread), SECRET),
        )
        .await,
        201,
    );
    let browser = full(
        create(
            &s,
            &s.ada.token,
            &once(&s, to_thread(browser_thread), SECRET),
        )
        .await,
        201,
    );
    assert_eq!(input_count(&s, keyed_thread).await, 0);
    assert_eq!(input_count(&s, browser_thread).await, 0);
    assert!(s.rig.turn_starts(keyed_thread).is_empty());
    s.rig.world.advance(Duration::minutes(2));
    assert_eq!(due(&s).await, 2);
    assert_eq!(due(&s).await, 0);
    let key_run = get(&s, keyed.id).await.last_run.unwrap();
    let browser_run = get(&s, browser.id).await.last_run.unwrap();
    let keyed_input = key_run
        .input_id
        .expect("the key's actual occurrence accepted one input");
    let browser_input = browser_run.input_id.unwrap();
    for (input, thread) in [(keyed_input, keyed_thread), (browser_input, browser_thread)] {
        let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.author_id, Some(s.ada.id()));
        assert_eq!(row.thread_id, thread);
        assert_eq!(row.content, json!([{"type":"text","text":SECRET}]));
        assert_eq!(input_count(&s, thread).await, 1);
    }
    let captured = input_authority(&s, keyed_input).await;
    let encoded = captured.to_string();
    assert!(encoded.contains(CREATE));
    assert!(encoded.contains(&author.id.to_string()));
    assert!(encoded.contains(&keyed.id.to_string()));
    assert!(encoded.contains(&key_run.id.to_string()));
    assert!(!encoded.contains("ThreadInputCreate"));
    assert!(!encoded.contains("node_ancestors"));
    for thread in [keyed_thread, browser_thread] {
        release(&s, thread).await;
        let start = s.rig.turn_start(thread, 0).await;
        assert!(start.text.ends_with(SECRET));
        s.rig
            .answer(thread, &start, "schedule-parity-answer", "done")
            .await;
        s.rig
            .say(
                thread,
                rig::turn_ended(start.turn_id, TurnOutcome::Completed),
            )
            .await;
        s.rig.wait_turn(start.turn_id, "completed").await;
    }
    assert_eq!(due(&s).await, 0);
    assert_eq!(input_count(&s, keyed_thread).await, 1);
    s.rig.engine.stop();
}

#[tokio::test]
async fn nonreading_schedule_keys_get_receipts_and_safe_stale_errors_not_saved_content() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    hold(&s, thread).await;
    let writer = key(
        &s,
        s.ada.id(),
        creation_rules(s.tree.topic.base.id, "thread", thread),
    )
    .await;
    let accepted = receipt(
        create(&s, &writer.secret, &once(&s, to_thread(thread), SECRET)).await,
        201,
    );
    denied(
        s.rig
            .world
            .app
            .call(InputScheduleGet, &[&accepted.id])
            .token(&writer.secret)
            .send_bytes()
            .await,
    );
    denied(
        s.rig
            .world
            .app
            .call(InputScheduleRunList, &[&accepted.id])
            .token(&writer.secret)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
    );
    denied(
        s.rig
            .world
            .app
            .call(TopicInputScheduleList, &[&s.tree.topic.base.id])
            .token(&writer.secret)
            .query(&InputScheduleListQuery::default())
            .send_bytes()
            .await,
    );
    let mut rules = editing_rules(accepted.id, thread);
    rules.push(rule(
        "InputScheduleGet",
        "schedule",
        accepted.id,
        &["schedule"],
        &["read"],
    ));
    let editor = key(&s, s.ada.id(), rules).await;
    let readable = s
        .rig
        .world
        .app
        .call(InputScheduleGet, &[&accepted.id])
        .token(&editor.secret)
        .send()
        .await
        .ok();
    assert!(readable.definition.is_some());
    let edited = receipt(
        update(
            &s,
            &editor.secret,
            accepted.id,
            &InputScheduleUpdateBody {
                revision: accepted.revision,
                name: Some("Private changed schedule label".into()),
                ..Default::default()
            },
        )
        .await,
        200,
    );
    assert!(edited.revision > accepted.revision);
    let before = schedule_state(&s, accepted.id).await;
    let stale = update(
        &s,
        &editor.secret,
        accepted.id,
        &InputScheduleUpdateBody {
            revision: accepted.revision,
            content: Some(vec![ContentPart::Text {
                text: "rejected replacement".into(),
            }]),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(stale.status.as_u16(), 409);
    assert_eq!(stale.problem().code, ErrorCode::ConflictStaleRevision);
    let raw = String::from_utf8_lossy(&stale.raw);
    for hidden in [SECRET, "Private changed schedule label", "Fix login"] {
        assert!(
            !raw.contains(hidden),
            "stale response disclosed {hidden}: {raw}"
        );
    }
    assert_eq!(schedule_state(&s, accepted.id).await, before);

    let mut readable_edit = editing_rules(accepted.id, thread);
    readable_edit.push(rule(
        EDIT,
        "schedule",
        accepted.id,
        &["schedule"],
        &["read"],
    ));
    let full_editor = key(&s, s.ada.id(), readable_edit).await;
    let changed = full(
        update(
            &s,
            &full_editor.secret,
            accepted.id,
            &InputScheduleUpdateBody {
                revision: edited.revision,
                name: Some("Readable current schedule label".into()),
                ..Default::default()
            },
        )
        .await,
        200,
    );
    assert_eq!(changed.id, accepted.id);
    assert_eq!(
        changed.definition.as_ref().unwrap().content,
        vec![ContentPart::Text {
            text: SECRET.into()
        }]
    );
    let current_state = schedule_state(&s, accepted.id).await;
    let full_stale = update(
        &s,
        &full_editor.secret,
        accepted.id,
        &InputScheduleUpdateBody {
            revision: edited.revision,
            name: Some("This stale name is not written".into()),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(full_stale.status.as_u16(), 409);
    assert_eq!(full_stale.problem().code, ErrorCode::ConflictStaleRevision);
    let problem: Value = serde_json::from_slice(&full_stale.raw).unwrap();
    let current: InputSchedule = serde_json::from_value(problem["current"].clone()).unwrap();
    assert_eq!(current.id, accepted.id);
    assert_eq!(current.revision, changed.revision);
    assert_eq!(current.name, changed.name);
    assert_eq!(current.definition, changed.definition);
    assert_eq!(schedule_state(&s, accepted.id).await, current_state);

    let mut exact_read = creation_rules(s.tree.topic.base.id, "thread", thread);
    exact_read.push(rule(
        CREATE,
        "topic",
        s.tree.topic.base.id,
        &["schedule"],
        &["read"],
    ));
    let exact_reader = key(&s, s.ada.id(), exact_read).await;
    receipt(
        create(
            &s,
            &exact_reader.secret,
            &once(&s, to_thread(thread), "Exact parent Read stays a receipt"),
        )
        .await,
        201,
    );
    let mut read_created = rule(
        CREATE,
        "topic",
        s.tree.topic.base.id,
        &["schedule"],
        &["read"],
    );
    read_created["target"]["reach"] = json!("subtree");
    let mut subtree_read = creation_rules(s.tree.topic.base.id, "thread", thread);
    subtree_read.push(read_created);
    let full_creator = key(&s, s.ada.id(), subtree_read).await;
    let created = full(
        create(
            &s,
            &full_creator.secret,
            &once(&s, to_thread(thread), "Readable created definition"),
        )
        .await,
        201,
    );
    assert_eq!(
        created.definition.unwrap().content,
        vec![ContentPart::Text {
            text: "Readable created definition".into()
        }]
    );
    assert_eq!(input_count(&s, thread).await, 0);
    s.rig.engine.stop();
}

#[tokio::test]
async fn schedule_readers_filter_actual_rows_and_keep_another_authors_definition_private() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    hold(&s, thread).await;
    let a = full(
        create(
            &s,
            &s.ada.token,
            &once(&s, to_thread(thread), "first private draft"),
        )
        .await,
        201,
    );
    let mut other = once(&s, to_thread(thread), "second private draft");
    other.name = Some("Second hidden schedule".into());
    let b = full(create(&s, &s.ada.token, &other).await, 201);
    let root_only = key(
        &s,
        s.ada.id(),
        vec![rule(
            "TopicInputScheduleList",
            "topic",
            s.tree.topic.base.id,
            &["topic"],
            &["read"],
        )],
    )
    .await;
    let empty: Page<InputSchedule> = s
        .rig
        .world
        .app
        .call(TopicInputScheduleList, &[&s.tree.topic.base.id])
        .token(&root_only.secret)
        .query(&InputScheduleListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        empty.items.is_empty(),
        "exact topic Read cannot expose child Schedule rows"
    );
    let reader = key(
        &s,
        s.dan.id(),
        vec![
            rule(
                "TopicInputScheduleList",
                "topic",
                s.tree.topic.base.id,
                &["topic"],
                &["read"],
            ),
            rule(
                "TopicInputScheduleList",
                "schedule",
                a.id,
                &["schedule"],
                &["read"],
            ),
            rule(
                "InputScheduleGet",
                "schedule",
                a.id,
                &["schedule"],
                &["read"],
            ),
            rule(
                "InputScheduleRunList",
                "schedule",
                a.id,
                &["schedule"],
                &["read"],
            ),
        ],
    )
    .await;
    let listed: Page<InputSchedule> = s
        .rig
        .world
        .app
        .call(TopicInputScheduleList, &[&s.tree.topic.base.id])
        .token(&reader.secret)
        .query(&InputScheduleListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        listed.items.iter().map(|row| row.id).collect::<Vec<_>>(),
        vec![a.id]
    );
    assert!(listed.items[0].definition.is_none());
    assert!(
        !serde_json::to_string(&listed)
            .unwrap()
            .contains("Fix login"),
        "Schedule Read does not silently include the destination's protected name"
    );
    let shown = s
        .rig
        .world
        .app
        .call(InputScheduleGet, &[&a.id])
        .token(&reader.secret)
        .send()
        .await
        .ok();
    assert!(shown.definition.is_none());
    let encoded = serde_json::to_string(&shown).unwrap();
    assert!(!encoded.contains("first private draft"));
    assert!(!encoded.contains("Fix login"));
    let named_reader = key(
        &s,
        s.dan.id(),
        vec![
            rule(
                "InputScheduleGet",
                "schedule",
                a.id,
                &["schedule"],
                &["read"],
            ),
            rule("InputScheduleGet", "thread", thread, &["thread"], &["read"]),
        ],
    )
    .await;
    let named = s
        .rig
        .world
        .app
        .call(InputScheduleGet, &[&a.id])
        .token(&named_reader.secret)
        .send()
        .await
        .ok();
    assert_eq!(
        named
            .destination
            .thread
            .as_ref()
            .and_then(|thread| thread.name.as_deref()),
        Some("Fix login")
    );
    assert!(named.definition.is_none());
    denied(
        s.rig
            .world
            .app
            .call(InputScheduleGet, &[&b.id])
            .token(&reader.secret)
            .send_bytes()
            .await,
    );
    s.rig.world.advance(Duration::minutes(2));
    assert_eq!(due(&s).await, 2);
    let runs: Page<InputScheduleRun> = s
        .rig
        .world
        .app
        .call(InputScheduleRunList, &[&a.id])
        .token(&reader.secret)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(runs.items.len(), 1);
    assert_eq!(runs.items[0].id, get(&s, a.id).await.last_run.unwrap().id);
    assert!(runs.items[0].input_id.is_some());
    assert!(!serde_json::to_string(&runs).unwrap().contains("Fix login"));
    assert!(
        !serde_json::to_string(&runs)
            .unwrap()
            .contains("private draft")
    );
    denied(
        s.rig
            .world
            .app
            .call(InputScheduleRunList, &[&b.id])
            .token(&reader.secret)
            .query(&PageQuery::default())
            .send_bytes()
            .await,
    );
    s.rig.engine.stop();
}

#[tokio::test]
async fn due_claims_recheck_original_key_revocation_expiry_and_current_destination_scope() {
    for change in ["revoke", "expire", "narrow"] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        hold(&s, thread).await;
        let author = key(
            &s,
            s.ada.id(),
            creation_rules(s.tree.topic.base.id, "thread", thread),
        )
        .await;
        let accepted = receipt(
            create(&s, &author.secret, &once(&s, to_thread(thread), SECRET)).await,
            201,
        );
        match change {
            "revoke" => revoke(&s, &author).await,
            "expire" => {
                sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
                    .bind(author.id)
                    .bind(s.rig.world.now() + Duration::seconds(30))
                    .execute(s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "narrow" => {
                policy(
                    &s,
                    &author,
                    vec![rule(
                        CREATE,
                        "topic",
                        s.tree.topic.base.id,
                        &["schedule"],
                        &["manage"],
                    )],
                )
                .await
            }
            _ => unreachable!(),
        }
        s.rig.world.advance(Duration::minutes(2));
        assert_eq!(due(&s).await, 1, "{change}");
        assert_eq!(due(&s).await, 0, "{change}");
        let finished = get(&s, accepted.id).await;
        let run = finished.last_run.unwrap();
        assert_eq!(run.state, InputScheduleRunState::Failed, "{change}");
        assert!(run.input_id.is_none(), "{change}");
        assert_eq!(input_count(&s, thread).await, 0, "{change}");
        assert!(s.rig.turn_starts(thread).is_empty(), "{change}");
        let notices: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='input_schedule.cannot_start'")
            .bind(accepted.id).fetch_one(s.rig.db.pool()).await.unwrap();
        assert_eq!(notices, 1, "{change}");
        s.rig.engine.stop();
    }
}

#[tokio::test]
async fn independent_create_and_definition_edit_keys_both_bound_the_actual_edited_occurrence() {
    for revoked in [None, Some("creator"), Some("editor")] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        hold(&s, thread).await;
        let creator = key(
            &s,
            s.ada.id(),
            creation_rules(s.tree.topic.base.id, "thread", thread),
        )
        .await;
        let accepted = receipt(
            create(
                &s,
                &creator.secret,
                &once(&s, to_thread(thread), "original future words"),
            )
            .await,
            201,
        );
        let editor = key(&s, s.ada.id(), editing_rules(accepted.id, thread)).await;
        receipt(
            update(
                &s,
                &editor.secret,
                accepted.id,
                &InputScheduleUpdateBody {
                    revision: accepted.revision,
                    content: Some(vec![ContentPart::Text {
                        text: "edited future words".into(),
                    }]),
                    ..Default::default()
                },
            )
            .await,
            200,
        );
        match revoked {
            Some("creator") => revoke(&s, &creator).await,
            Some("editor") => revoke(&s, &editor).await,
            None => {}
            _ => unreachable!(),
        }
        s.rig.world.advance(Duration::minutes(2));
        assert_eq!(due(&s).await, 1, "{revoked:?}");
        assert_eq!(due(&s).await, 0, "{revoked:?}");
        let run = get(&s, accepted.id).await.last_run.unwrap();
        if revoked.is_some() {
            assert_eq!(run.state, InputScheduleRunState::Failed, "{revoked:?}");
            assert!(run.input_id.is_none());
            assert_eq!(input_count(&s, thread).await, 0);
        } else {
            let input = run
                .input_id
                .expect("both distinct operation origins admit the edited occurrence");
            let captured = input_authority(&s, input).await.to_string();
            for retained in [
                CREATE,
                EDIT,
                &creator.id.to_string(),
                &editor.id.to_string(),
            ] {
                assert!(
                    captured.contains(retained),
                    "missing retained origin {retained}: {captured}"
                );
            }
            assert!(!captured.contains("ThreadInputCreate"));
            release(&s, thread).await;
            let start = s.rig.turn_start(thread, 0).await;
            assert!(start.text.ends_with("edited future words"));
            assert!(!start.text.contains("original future words"));
            s.rig
                .answer(thread, &start, "edited-schedule-answer", "done")
                .await;
            s.rig
                .say(
                    thread,
                    rig::turn_ended(start.turn_id, TurnOutcome::Completed),
                )
                .await;
            s.rig.wait_turn(start.turn_id, "completed").await;
        }
        s.rig.engine.stop();
    }
}

async fn let_use_model(s: &Scene, person: &places::Who) {
    for (kind, id, permission) in [
        ("agent", s.on.agent, "agent.use"),
        ("provider", s.on.provider, "provider.use"),
        ("environment", s.on.environment, "environment.use"),
    ] {
        s.rig
            .lets_use(kind, id, permission, person, json!({}))
            .await;
    }
}

async fn let_interact(s: &Scene, thread: Id, person: Id) -> Id {
    let mut conn = s.rig.world.conn().await;
    let entry = orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", thread, "live"),
        "any",
        "thread.interact",
        ("user", Some(person), None),
        None,
        s.rig.world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    entry
}

#[tokio::test]
async fn another_controller_resume_keeps_the_author_boundary_and_delete_keeps_accepted_work() {
    for revoke_author in [false, true] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        hold(&s, thread).await;
        let_use_model(&s, &s.dan).await;
        let_interact(&s, thread, s.dan.id()).await;
        let author = key(
            &s,
            s.ada.id(),
            creation_rules(s.tree.topic.base.id, "thread", thread),
        )
        .await;
        let accepted = receipt(
            create(&s, &author.secret, &once(&s, to_thread(thread), SECRET)).await,
            201,
        );
        let controller = key(
            &s,
            s.dan.id(),
            vec![rule(
                STATE,
                "schedule",
                accepted.id,
                &["schedule"],
                &["manage"],
            )],
        )
        .await;
        let paused = receipt(
            update(
                &s,
                &controller.secret,
                accepted.id,
                &InputScheduleUpdateBody {
                    revision: accepted.revision,
                    state: Some(InputScheduleState::Paused),
                    ..Default::default()
                },
            )
            .await,
            200,
        );
        let resumed = receipt(
            update(
                &s,
                &controller.secret,
                accepted.id,
                &InputScheduleUpdateBody {
                    revision: paused.revision,
                    state: Some(InputScheduleState::Active),
                    ..Default::default()
                },
            )
            .await,
            200,
        );
        assert!(resumed.revision > paused.revision);
        // Successful resume has already passed the controller's ordinary model-use conditions.
        // Only accepted author definitions may remain as dependencies of the future run.
        let captured: Value =
            sqlx::query_scalar("SELECT definition_authority FROM input_schedules WHERE id=$1")
                .bind(accepted.id)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        assert!(captured.to_string().contains(&author.id.to_string()));
        assert!(!captured.to_string().contains(&controller.id.to_string()));
        if revoke_author {
            revoke(&s, &author).await;
        } else {
            revoke(&s, &controller).await;
        }
        s.rig.world.advance(Duration::minutes(2));
        assert_eq!(due(&s).await, 1);
        let run = get(&s, accepted.id).await.last_run.unwrap();
        if revoke_author {
            assert_eq!(run.state, InputScheduleRunState::Failed);
            assert!(
                run.input_id.is_none(),
                "the controller cannot replace a revoked author"
            );
            assert_eq!(input_count(&s, thread).await, 0);
        } else {
            let input = run.input_id.expect(
                "revoking a completed controller act does not revoke the author's future work",
            );
            let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.author_id, Some(s.ada.id()));
            assert!(
                !row.submission_authority
                    .unwrap()
                    .to_string()
                    .contains(&controller.id.to_string())
            );
        }
        let remover = key(
            &s,
            s.dan.id(),
            vec![rule(
                "InputScheduleDelete",
                "schedule",
                accepted.id,
                &["schedule"],
                &["manage"],
            )],
        )
        .await;
        for _ in 0..2 {
            let reply = s
                .rig
                .world
                .app
                .call(InputScheduleDelete, &[&accepted.id])
                .token(&remover.secret)
                .send_bytes()
                .await;
            assert_eq!(
                reply.status.as_u16(),
                204,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert!(reply.raw.is_empty());
        }
        if let Some(input) = run.input_id {
            assert!(
                orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
                    .await
                    .unwrap()
                    .is_some(),
                "deleting future scheduling does not delete its accepted input"
            );
        }
        s.rig.engine.stop();
    }
}

fn new_thread(s: &Scene) -> InputScheduleTarget {
    InputScheduleTarget::NewThread {
        config: Box::new(CreateThread {
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(rig::MODEL.into()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            ..Default::default()
        }),
    }
}

async fn thread_count(s: &Scene) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM threads WHERE topic_id=$1")
        .bind(s.tree.topic.base.id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn new_thread_schedules_bind_the_actual_parent_and_create_nothing_after_scope_loss() {
    let s = Scene::new().await;
    let existing = s.thread().await.base.id;
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let before = thread_count(&s).await;
    let wrong = key(
        &s,
        s.ada.id(),
        creation_rules(s.tree.topic.base.id, "thread", existing),
    )
    .await;
    denied(create(&s, &wrong.secret, &once(&s, new_thread(&s), SECRET)).await);
    assert_eq!(thread_count(&s).await, before);
    let permitted = key(
        &s,
        s.ada.id(),
        creation_rules(s.tree.topic.base.id, "topic", s.tree.topic.base.id),
    )
    .await;
    let accepted = receipt(
        create(&s, &permitted.secret, &once(&s, new_thread(&s), SECRET)).await,
        201,
    );
    let narrowing = key(
        &s,
        s.ada.id(),
        creation_rules(s.tree.topic.base.id, "topic", s.tree.topic.base.id),
    )
    .await;
    let mut other = once(&s, new_thread(&s), "this future thread must never exist");
    other.name = Some("Narrowed future thread".into());
    let denied_later = receipt(create(&s, &narrowing.secret, &other).await, 201);
    assert_eq!(
        thread_count(&s).await,
        before,
        "definition validation leaves no trial thread"
    );
    policy(
        &s,
        &narrowing,
        vec![rule(
            CREATE,
            "topic",
            s.tree.topic.base.id,
            &["schedule"],
            &["manage"],
        )],
    )
    .await;
    s.rig.world.advance(Duration::minutes(2));
    assert_eq!(due(&s).await, 2);
    assert_eq!(due(&s).await, 0);
    let run = get(&s, accepted.id).await.last_run.unwrap();
    let actual = run.thread.unwrap().id;
    assert_ne!(actual, existing);
    let input = run
        .input_id
        .expect("the new thread and its first input commit together");
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.thread_id, actual);
    assert_eq!(row.author_id, Some(s.ada.id()));
    let captured = row.submission_authority.unwrap().to_string();
    assert!(
        captured.contains(CREATE)
            && captured.contains(&accepted.id.to_string())
            && captured.contains(&run.id.to_string())
    );
    let failed = get(&s, denied_later.id).await.last_run.unwrap();
    assert_eq!(failed.state, InputScheduleRunState::Failed);
    assert!(failed.input_id.is_none() && failed.thread.is_none());
    assert_eq!(
        thread_count(&s).await,
        before + 1,
        "scope loss cannot leave an orphan trial thread"
    );
    assert_eq!(input_count(&s, actual).await, 1);
    s.rig.engine.stop();
}

#[tokio::test]
async fn a_valid_key_does_not_replace_the_authors_lost_ordinary_interact_permission() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    hold(&s, thread).await;
    let_use_model(&s, &s.cai).await;
    let interact = let_interact(&s, thread, s.cai.id()).await;
    let author = key(
        &s,
        s.cai.id(),
        creation_rules(s.tree.topic.base.id, "thread", thread),
    )
    .await;
    let accepted = receipt(
        create(&s, &author.secret, &once(&s, to_thread(thread), SECRET)).await,
        201,
    );
    sqlx::query("UPDATE permission_entries SET expires_at=$2 WHERE id=$1")
        .bind(interact)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    orch_db::queries::access::bump_epoch(&mut *s.rig.world.conn().await)
        .await
        .unwrap();
    s.rig.world.advance(Duration::minutes(2));
    let valid: bool = sqlx::query_scalar("SELECT t.revoked_at IS NULL AND (t.expires_at IS NULL OR t.expires_at>$2) AND u.status='active' FROM api_tokens t JOIN users u ON u.id=t.user_id WHERE t.id=$1")
        .bind(author.id).bind(s.rig.world.now()).fetch_one(s.rig.db.pool()).await.unwrap();
    assert!(valid, "only the ordinary permission was removed");
    assert_eq!(due(&s).await, 1);
    assert_eq!(due(&s).await, 0);
    let run = get(&s, accepted.id).await.last_run.unwrap();
    assert_eq!(run.state, InputScheduleRunState::Failed);
    assert!(run.input_id.is_none());
    assert_eq!(input_count(&s, thread).await, 0);
    assert!(s.rig.turn_starts(thread).is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn a_scheduled_attachment_needs_its_accepted_keys_current_readable_byte_witness() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    hold(&s, thread).await;
    let bytes = bytes::Bytes::from_static(b"Actual retained bytes for the future scheduled input");
    let expected_hash = orch_types::Hash::of(&bytes);
    let stored = s.rig.world.app.app.blobs.put_bytes(bytes).await.unwrap();
    assert_eq!(stored.hash, expected_hash);
    let mut conn = s.rig.world.conn().await;
    s.rig
        .world
        .app
        .app
        .blobs
        .register(&mut conn, stored, s.rig.world.now())
        .await
        .unwrap();
    orch_db::queries::uploads::remember(&mut conn, s.ada.id(), &stored.hash, s.rig.world.now())
        .await
        .unwrap();
    drop(conn);
    let mut body = once(&s, to_thread(thread), SECRET);
    body.content.push(ContentPart::File {
        path: ".orch/attachments/scheduled.txt".into(),
        name: "scheduled.txt".into(),
        size: stored.size,
        blob: Some(stored.hash),
    });
    let base = creation_rules(s.tree.topic.base.id, "thread", thread);
    let no_read = key(&s, s.ada.id(), base.clone()).await;
    let refused = create(&s, &no_read.secret, &body).await;
    assert_eq!(refused.status.as_u16(), 404);
    assert_eq!(refused.problem().code, ErrorCode::NotFound);
    let mut readable = base.clone();
    readable.push(rule(CREATE, "account", s.ada.id(), &["upload"], &["read"]));
    let author = key(&s, s.ada.id(), readable).await;
    let accepted = receipt(create(&s, &author.secret, &body).await, 201);
    assert_eq!(
        get(&s, accepted.id).await.definition.unwrap().content,
        body.content
    );
    assert_eq!(input_count(&s, thread).await, 0);
    // The bytes and ordinary owner remain; only this operation's current Read witness is gone.
    policy(&s, &author, base).await;
    s.rig.world.advance(Duration::minutes(2));
    assert_eq!(due(&s).await, 1);
    assert_eq!(due(&s).await, 0);
    let run = get(&s, accepted.id).await.last_run.unwrap();
    assert_eq!(run.state, InputScheduleRunState::Failed);
    assert!(run.input_id.is_none());
    assert_eq!(input_count(&s, thread).await, 0);
    assert!(s.rig.turn_starts(thread).is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn a_scheduled_native_manage_selection_needs_manage_at_acceptance_and_when_due() {
    use orch_api_contract::native::*;
    use orch_api_contract::threads::Delivery;

    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.say(
        &s.ada,
        thread,
        "Open the native schedule session",
        Delivery::Queue,
        "native-schedule-open",
    )
    .await
    .ok();
    s.rig.run_turn(thread, 0, "ready").await;
    hold(&s, thread).await;
    *s.rig.commands.native.lock().unwrap() = Some(NativeCatalog {
        revision: "scheduled-native-catalogue".into(),
        state: NativeCatalogState::Ready,
        actions: vec![NativeAction {
            id: "scheduled-manage-artifact".into(),
            name: "scheduled-context-action".into(),
            title: "Scheduled context action".into(),
            kind: NativeActionKind::Skill,
            description: "Current stored action with ordinary Manage permission".into(),
            arguments_schema: json!({"type":"object","additionalProperties":false}),
            argument_hint: None,
            origin: NativeOrigin {
                kind: NativeOriginKind::Builtin,
                name: None,
                path: None,
            },
            permission: NativePermission::Manage,
            delivery_modes: vec![NativeDelivery::Queue],
            available: true,
            reason: None,
        }],
        reason: None,
        project_skills: NativeProjectSkills {
            available: false,
            reason: Some("No project discovery in this fixture".into()),
        },
        as_of: None,
    });
    let catalog = s
        .rig
        .world
        .app
        .call(ThreadNativeActionList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadNativeActionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(catalog.actions[0].permission, NativePermission::Manage);
    let mut body = once(&s, to_thread(thread), SECRET);
    body.native_action = Some(NativeInvocation {
        action_id: catalog.actions[0].id.clone(),
        revision: catalog.revision,
        arguments: json!({}),
    });
    let base = creation_rules(s.tree.topic.base.id, "thread", thread);
    let plain = key(&s, s.ada.id(), base.clone()).await;
    let before = input_count(&s, thread).await;
    denied(create(&s, &plain.secret, &body).await);
    assert_eq!(input_count(&s, thread).await, before);
    let mut management = base.clone();
    management.push(rule(CREATE, "thread", thread, &["thread"], &["manage"]));
    let author = key(&s, s.ada.id(), management).await;
    let accepted = receipt(create(&s, &author.secret, &body).await, 201);
    assert_eq!(
        get(&s, accepted.id).await.definition.unwrap().native_action,
        body.native_action
    );
    assert_eq!(
        input_count(&s, thread).await,
        before,
        "saving the selection invokes nothing"
    );
    policy(&s, &author, base).await;
    s.rig.world.advance(Duration::minutes(2));
    assert_eq!(due(&s).await, 1);
    assert_eq!(due(&s).await, 0);
    let run = get(&s, accepted.id).await.last_run.unwrap();
    assert_eq!(run.state, InputScheduleRunState::Failed);
    assert!(run.input_id.is_none());
    assert_eq!(input_count(&s, thread).await, before);
    assert_eq!(
        s.rig.turn_starts(thread).len(),
        1,
        "only the ordinary setup turn ran"
    );
    s.rig.engine.stop();
}

// These continuation controls retain the real occurrence/input. They do not manufacture
// a browser caller or treat a public command's completion as proof of first-send authority.
fn source_rules(operation: &str, schedule: Option<Id>, s: &Scene) -> Vec<Value> {
    let mut rules = if let Some(schedule) = schedule {
        vec![
            rule(operation, "schedule", schedule, &["schedule"], &["manage"]),
            rule(
                operation,
                "topic",
                s.tree.topic.base.id,
                &["thread"],
                &["write"],
            ),
        ]
    } else {
        creation_rules(s.tree.topic.base.id, "topic", s.tree.topic.base.id)
    };
    rules.push(rule(
        operation,
        "environment",
        s.on.environment,
        &["environment"],
        &["manage"],
    ));
    rules
}

fn source_destination(s: &Scene) -> InputScheduleTarget {
    let mut target = new_thread(s);
    let InputScheduleTarget::NewThread { config } = &mut target else {
        unreachable!()
    };
    config.directory = Some("scheduled-source".into());
    config.prepare_source = Some(true);
    target
}

async fn accepted_source(s: &Scene) -> (Receipt, Key, Key, Id, Id) {
    s.rig.engine.stop();
    let creator = key(s, s.ada.id(), source_rules(CREATE, None, s)).await;
    let accepted = receipt(
        create(s, &creator.secret, &once(s, source_destination(s), SECRET)).await,
        201,
    );
    let editor = key(s, s.ada.id(), source_rules(EDIT, Some(accepted.id), s)).await;
    receipt(
        update(
            s,
            &editor.secret,
            accepted.id,
            &InputScheduleUpdateBody {
                revision: accepted.revision,
                content: Some(vec![ContentPart::Text {
                    text: "Edited source occurrence".into(),
                }]),
                ..Default::default()
            },
        )
        .await,
        200,
    );
    s.rig.world.advance(Duration::minutes(2));
    assert_eq!(due(s).await, 1);
    let run = get(s, accepted.id).await.last_run.unwrap();
    let thread = run.thread.as_ref().unwrap().id;
    let input = run.input_id.unwrap();
    let accepted_input = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, input)
        .await
        .unwrap()
        .unwrap();
    let access = orch_api_core::thread_access::AuthzAccess::new(std::sync::Arc::clone(
        &s.rig.world.app.app.clock,
    ));
    let ordinary_use = orch_engine::access::ResourceAccess::provider_use(
        &access,
        &mut *s.rig.world.conn().await,
        &orch_engine::access::ProviderUse {
            purpose: orch_access_contract::permissions::ProviderPurpose::Turn,
            person: s.ada.id(),
            node: thread,
            provider: s.on.provider,
            model: Some(rig::MODEL),
            effort: None,
            runs_in: None,
        },
    )
    .await
    .unwrap();
    assert!(
        ordinary_use.is_allowed(),
        "the source hold must not conceal a missing ordinary resource use: {ordinary_use:?}"
    );
    // submit_input runs pre-flight synchronously even while the supervisor is stopped.
    // An explicitly pending source keeps the accepted input at the ordinary files gate.
    assert_eq!(run.state, InputScheduleRunState::Blocked);
    assert_eq!(run.reason.as_deref(), Some("files"));
    assert_eq!(accepted_input.thread_id, thread);
    assert_eq!(accepted_input.author_id, Some(s.ada.id()));
    assert_eq!(accepted_input.state, "pending");
    assert_eq!(accepted_input.state_detail.as_deref(), Some("files"));
    assert!(accepted_input.turn_id.is_none() && run.turn_id.is_none());
    let row = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, thread)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "starting");
    assert_eq!(row.starting_step.as_deref(), Some("source"));
    let source = row.source.unwrap();
    let info: orch_api_contract::source::SourceInfo =
        serde_json::from_value(source["info"].clone()).unwrap();
    assert_eq!(info.state, orch_api_contract::source::SourceState::Pending);
    assert_eq!(source["scheduled_input"], json!(input));
    assert!(
        source["scheduled_environment_generation"]
            .as_i64()
            .is_some()
    );
    let captured = input_authority(s, input).await.to_string();
    for identity in [
        CREATE.to_owned(),
        EDIT.to_owned(),
        creator.id.to_string(),
        editor.id.to_string(),
        run.id.to_string(),
    ] {
        assert!(captured.contains(&identity), "{captured}");
    }
    assert_eq!(
        s.rig.commands.issued_of("fs.stat"),
        0,
        "source preparation cannot run before the due transaction is committed and its job is claimed"
    );
    (accepted, creator, editor, thread, input)
}

async fn source_authorized(s: &Scene, thread: Id, input: Id) -> bool {
    orch_engine::inputs::scheduled_source_authorized(
        &mut *s.rig.world.conn().await,
        input,
        thread,
        s.on.environment,
        false,
        Time::new(s.rig.world.now()),
        s.ada.id(),
        None,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn accepted_schedule_source_keeps_all_origins_after_delete_and_rejects_wrong_anchors() {
    let s = Scene::new().await;
    let (schedule, _creator, _editor, thread, input) = accepted_source(&s).await;
    assert!(source_authorized(&s, thread, input).await);
    s.rig
        .world
        .app
        .call(InputScheduleDelete, &[&schedule.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert!(
        source_authorized(&s, thread, input).await,
        "deleting future occurrences does not revoke already accepted work"
    );
    let unrelated = s.thread().await.base.id;
    assert!(!source_authorized(&s, unrelated, input).await);
    assert!(!source_authorized(&s, thread, new_id()).await);
    let mut tx = s.rig.world.app.app.db.begin().await.unwrap();
    let changed = sqlx::query("UPDATE input_schedule_runs SET input_id=NULL WHERE input_id=$1")
        .bind(input)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    assert!(
        !orch_engine::inputs::scheduled_source_authorized(
            &mut tx,
            input,
            thread,
            s.on.environment,
            false,
            Time::new(s.rig.world.now()),
            s.ada.id(),
            None,
        )
        .await
        .unwrap()
    );
    tx.rollback().await.unwrap();
    assert!(source_authorized(&s, thread, input).await);
}

#[tokio::test]
async fn scheduled_source_continuation_rechecks_each_key_file_scope_and_accepted_generation() {
    for lost in ["creator", "editor", "files", "generation", "ordinary"] {
        let s = Scene::new().await;
        let (_schedule, creator, editor, thread, input) = accepted_source(&s).await;
        assert!(source_authorized(&s, thread, input).await, "{lost}");
        match lost {
            "creator" => revoke(&s, &creator).await,
            "editor" => revoke(&s, &editor).await,
            "files" => {
                policy(
                    &s,
                    &creator,
                    creation_rules(s.tree.topic.base.id, "topic", s.tree.topic.base.id),
                )
                .await
            }
            "generation" => {
                sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
                    .bind(s.on.environment)
                    .execute(s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "ordinary" => {
                orch_db::queries::access::testing::suspend_in(
                    &mut *s.rig.world.conn().await,
                    s.tree.org.base.id,
                    s.ada.id(),
                    s.ada.id(),
                )
                .await
                .unwrap();
                orch_db::queries::access::bump_epoch(&mut *s.rig.world.conn().await)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(!source_authorized(&s, thread, input).await, "{lost}");
        assert_eq!(s.rig.commands.issued_of("fs.stat"), 0);
        assert_eq!(s.rig.commands.issued_of("git.worktree"), 0);
    }
}

#[tokio::test]
async fn scheduled_source_scope_is_required_at_acceptance_and_actual_materialization() {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let insufficient = key(
        &s,
        s.ada.id(),
        creation_rules(s.tree.topic.base.id, "topic", s.tree.topic.base.id),
    )
    .await;
    let before = thread_count(&s).await;
    denied(
        create(
            &s,
            &insufficient.secret,
            &once(&s, source_destination(&s), SECRET),
        )
        .await,
    );
    assert_eq!(thread_count(&s).await, before);
    assert_eq!(s.rig.commands.issued_of("fs.stat"), 0);
    let enough = key(&s, s.ada.id(), source_rules(CREATE, None, &s)).await;
    let accepted = receipt(
        create(
            &s,
            &enough.secret,
            &once(&s, source_destination(&s), SECRET),
        )
        .await,
        201,
    );
    policy(
        &s,
        &enough,
        creation_rules(s.tree.topic.base.id, "topic", s.tree.topic.base.id),
    )
    .await;
    s.rig.world.advance(Duration::minutes(2));
    assert_eq!(due(&s).await, 1);
    let run = get(&s, accepted.id).await.last_run.unwrap();
    assert_eq!(run.state, InputScheduleRunState::Failed);
    assert!(run.thread.is_none() && run.input_id.is_none());
    assert_eq!(
        thread_count(&s).await,
        before,
        "scope refusal rolls back the ordinary thread/source prefix"
    );
    assert_eq!(s.rig.commands.issued_of("fs.stat"), 0);
}

#[tokio::test]
async fn a_queued_scheduled_file_command_uses_the_same_all_origin_wire_checkpoint() {
    use orch_api_contract::source::{RepositorySource, SourceDestination};
    use orch_engine::jobs::source::{self, Action, Plan};
    use orch_link_contract::commands::{FsStat, SerialKey};
    let s = Scene::new().await;
    let (_schedule, _creator, editor, thread, input) = accepted_source(&s).await;
    let mut tx = s.rig.world.app.app.db.begin().await.unwrap();
    let row = orch_db::queries::threads::thread(&mut tx, thread)
        .await
        .unwrap()
        .unwrap();
    let frozen = row.source.as_ref().unwrap();
    let environment =
        orch_db::queries::environments::workspace::environment(&mut tx, s.on.environment)
            .await
            .unwrap()
            .unwrap();
    let job = source::begin(
        &mut tx,
        Plan {
            environment: s.on.environment,
            generation: environment.generation,
            machine: environment.machine_id.unwrap(),
            thread: Some(thread),
            caller: s.ada.id(),
            token: None,
            creator_origin: serde_json::from_value(
                frozen.get("creator_origin").cloned().unwrap_or(Value::Null),
            )
            .unwrap(),
            scheduled_input: Some(input),
            action: Action::Prepare {
                directory: row.workspace_dir.clone(),
                source: serde_json::from_value::<RepositorySource>(frozen["source"].clone())
                    .unwrap(),
                credential: None,
                destination: SourceDestination::MissingDirectory,
                effect_id: None,
            },
        },
        s.rig.world.now(),
    )
    .await
    .unwrap();
    let plan: Plan = serde_json::from_value(job.plan.clone()).unwrap();
    let command = new_id();
    let body = json!(FsStat {
        environment_id: plan.environment,
        mount: None,
        path: row.workspace_dir
    });
    orch_db::queries::jobs::merge_plan(
        &mut tx,
        job.id,
        &json!({
            "active_command":{"id":command,"machine":plan.machine,"kind":"fs.stat"},
            "creator_command":{"id":command,"plan":plan,"runtime":null},
        }),
        s.rig.world.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let serial = SerialKey::Environment(plan.environment).wire();
    assert!(
        source::creator_command_authorized(
            &mut *s.rig.world.conn().await,
            command,
            plan.machine,
            serial.as_deref(),
            "fs.stat",
            &body,
            Time::new(s.rig.world.now()),
        )
        .await
        .unwrap(),
        "the actual queued source recipe is admitted while every definition origin remains valid"
    );
    revoke(&s, &editor).await;
    assert!(
        !source::creator_command_authorized(
            &mut *s.rig.world.conn().await,
            command,
            plan.machine,
            serial.as_deref(),
            "fs.stat",
            &body,
            Time::new(s.rig.world.now()),
        )
        .await
        .unwrap(),
        "an unacknowledged durable file command cannot lose the separate editor origin at first transmission"
    );
    assert_eq!(s.rig.commands.issued_of("fs.stat"), 0);
    assert_eq!(s.rig.commands.issued_of("git.worktree"), 0);
}
