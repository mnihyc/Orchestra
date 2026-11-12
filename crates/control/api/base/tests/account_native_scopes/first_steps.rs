//! Setup facts retain their native observations without borrowing other operations' reads.

use super::{
    authority::{home, rule},
    checked, key, wait_for_activity,
};
use crate::{desk::Desk, places, support::World, usage_world::team};
use http::StatusCode;
use orch_api_contract::first_steps::{FirstStepsGetQuery, MeFirstStepsGet};
use orch_types::{Id, Json, new_id};
use serde_json::json;

fn step<'a>(body: &'a Json, id: &str) -> &'a Json {
    body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["id"] == id)
        .unwrap()
}

#[tokio::test]
async fn first_steps_keeps_native_empty_and_full_shapes_and_requires_its_actual_account_and_org() {
    let world = World::new().await;
    let actor = places::person(&world, "first-scope").await;
    let org = places::make_org(&world, &actor, "Scoped first team", "first-scope-team")
        .await
        .base
        .id;
    let query = FirstStepsGetQuery { org_id: Some(org) };
    let native = world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&actor.token)
        .query(&query)
        .send()
        .await
        .ok();
    let token = key(
        &world,
        actor.id(),
        vec![
            rule("MeFirstStepsGet", "account", actor.id()),
            home("MeFirstStepsGet", org),
        ],
    )
    .await;
    let body = checked(
        world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(body, serde_json::to_value(native).unwrap());
    assert_eq!(step(&body, "provider")["fact"], "not_done");
    assert_eq!(step(&body, "work")["fact"], "not_done");
    for rules in [
        vec![home("MeFirstStepsGet", org)],
        vec![
            rule("MeGet", "account", actor.id()),
            home("MeFirstStepsGet", org),
        ],
        vec![
            rule("MeFirstStepsGet", "account", actor.id()),
            home("ProfileGet", org),
        ],
    ] {
        let token = key(&world, actor.id(), rules).await;
        checked(
            world
                .app
                .call(MeFirstStepsGet, &[])
                .token(&token)
                .query(&query)
                .send_bytes()
                .await,
            StatusCode::FORBIDDEN,
        );
    }
    let token = key(
        &world,
        actor.id(),
        vec![rule("MeFirstStepsGet", "account", actor.id())],
    )
    .await;
    let none = checked(
        world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&FirstStepsGetQuery::default())
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert!(none["org"].is_null());
    assert_eq!(none["steps"], json!([]));
    let stranger = places::person(&world, "first-stranger").await;
    let token = key(
        &world,
        stranger.id(),
        vec![
            rule("MeFirstStepsGet", "account", stranger.id()),
            home("MeFirstStepsGet", org),
        ],
    )
    .await;
    checked(
        world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::FORBIDDEN,
    );
}

#[tokio::test]
async fn first_steps_provider_and_model_observations_need_the_same_operation_read_before_their_urls()
 {
    let world = World::new().await;
    let t = team(&world).await;
    orch_db::queries::resources::access_fixtures::insert_model(
        &mut *world.conn().await,
        t.provider,
        "first-native-model",
        &["low"],
        true,
        t.boss.id(),
    )
    .await
    .unwrap();
    let query = FirstStepsGetQuery {
        org_id: Some(t.acme),
    };
    for extra in [Vec::new(), vec![rule("ProfileGet", "provider", t.provider)]] {
        let token = key(
            &world,
            t.boss.id(),
            vec![
                rule("MeFirstStepsGet", "account", t.boss.id()),
                rule("MeFirstStepsGet", "org", t.acme),
            ]
            .into_iter()
            .chain(extra)
            .collect(),
        )
        .await;
        let body = checked(
            world
                .app
                .call(MeFirstStepsGet, &[])
                .token(&token)
                .query(&query)
                .send_bytes()
                .await,
            StatusCode::OK,
        );
        for id in ["provider", "models"] {
            assert_eq!(step(&body, id)["fact"], "unknown");
            assert_eq!(step(&body, id)["state"], "unknown");
            assert!(
                !step(&body, id)["action"]["url"]
                    .as_str()
                    .unwrap()
                    .contains(&t.provider.to_string())
            );
        }
    }
    let token = key(
        &world,
        t.boss.id(),
        vec![
            rule("MeFirstStepsGet", "account", t.boss.id()),
            home("MeFirstStepsGet", t.acme),
        ],
    )
    .await;
    let body = checked(
        world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(step(&body, "provider")["fact"], "key_saved");
    assert_eq!(step(&body, "models")["fact"], "models_enabled");
    assert!(
        step(&body, "provider")["action"]["url"]
            .as_str()
            .unwrap()
            .contains(&t.provider.to_string())
    );
    assert!(!body.to_string().contains("sk-test"));
    let native = world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&t.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(body, serde_json::to_value(native).unwrap());
}

#[tokio::test]
async fn first_steps_preserves_the_dormant_start_here_without_inventing_a_document_source() {
    // The native optional source is dormant until this setting has a registry declaration.
    assert!(orch_settings_contract::registry::get("org.start_here_document_id").is_none());
    let desk = Desk::new().await;
    desk.make(&desk.ada, "An unrelated native document").await;
    let query = FirstStepsGetQuery {
        org_id: Some(desk.org),
    };
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![
            rule("MeFirstStepsGet", "account", desk.ada.id()),
            rule("MeFirstStepsGet", "org", desk.org),
        ],
    )
    .await;
    let body = checked(
        desk.world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert!(body["start_here"].is_null());
    let native = desk
        .world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&desk.ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(body, serde_json::to_value(native).unwrap());
}

async fn completed_answer(desk: &Desk, harness: &str) -> Id {
    use orch_db::queries::threads::{self, NewInput, NewTurn, Place};
    let (thread, segment) =
        crate::search_world::thread(desk, &desk.ada, desk.topic, "Observed answer").await;
    let (input, turn) = (new_id(), new_id());
    let mut conn = desk.world.conn().await;
    // The search helper creates a fake agent. This observation uses the segment's
    // actual harness identity, independently of its model name or answer words.
    let changed = sqlx::query(
        "UPDATE agents a SET harness_kind=$2 FROM segments s WHERE s.id=$1 AND a.id=s.agent_id",
    )
    .bind(segment)
    .bind(harness)
    .execute(&mut *conn)
    .await
    .unwrap();
    assert_eq!(changed.rows_affected(), 1);
    threads::insert_input(
        &mut conn,
        &NewInput {
            id: input,
            thread_id: thread,
            author_id: Some(desk.ada.id()),
            submission_authority: Some(
                &json!({"credentials":[{"type":"session","as_admin":false}]}),
            ),
            run_as: None,
            run_as_authority: None,
            source: "user",
            content: &json!([{"type":"text","text":"Native first answer"}]),
            delivery: "queue",
            state: "pending",
            turn_id: None,
            client_id: None,
            limit_bypass: None,
            place: Place::End,
            plan: false,
            at: desk.world.now(),
        },
        2,
    )
    .await
    .unwrap()
    .unwrap();
    threads::insert_turn(
        &mut conn,
        &NewTurn {
            id: turn,
            thread_id: thread,
            segment_id: segment,
            input_id: input,
            model: "native-first-answer",
            effort: None,
            params: &json!({}),
            lease_id: None,
            limit_bypass: None,
            at: desk.world.now(),
        },
        3,
    )
    .await
    .unwrap();
    threads::finish_turn(&mut conn, turn, "completed", None, desk.world.now(), 4)
        .await
        .unwrap();
    let item = new_id();
    sqlx::query("INSERT INTO items (id,thread_id,segment_id,turn_id,stream,kind,status,payload,seq_created,seq_updated) VALUES ($1,$2,$3,$4,'main','agent_message','completed',$5,5,5)")
        .bind(item).bind(thread).bind(segment).bind(turn).bind(json!({"text":"A genuine native completed answer","text_len":32,"phase":"final"})).execute(&mut *conn).await.unwrap();
    thread
}

#[tokio::test]
async fn first_steps_answer_uses_the_actual_thread_and_preserves_its_native_answer_filter() {
    let desk = Desk::new().await;
    let thread = completed_answer(&desk, "codex").await;
    let query = FirstStepsGetQuery {
        org_id: Some(desk.org),
    };
    for extra in [Vec::new(), vec![rule("ProfileGet", "thread", thread)]] {
        let token = key(
            &desk.world,
            desk.ada.id(),
            vec![
                rule("MeFirstStepsGet", "account", desk.ada.id()),
                rule("MeFirstStepsGet", "org", desk.org),
            ]
            .into_iter()
            .chain(extra)
            .collect(),
        )
        .await;
        let body = checked(
            desk.world
                .app
                .call(MeFirstStepsGet, &[])
                .token(&token)
                .query(&query)
                .send_bytes()
                .await,
            StatusCode::OK,
        );
        assert_eq!(step(&body, "work")["fact"], "unknown");
        assert!(!body.to_string().contains(&thread.to_string()));
        assert!(
            body["steps"]
                .as_array()
                .unwrap()
                .iter()
                .all(|step| step["id"] != "away")
        );
    }
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![
            rule("MeFirstStepsGet", "account", desk.ada.id()),
            home("MeFirstStepsGet", desk.org),
        ],
    )
    .await;
    let body = checked(
        desk.world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(step(&body, "work")["fact"], "answered_once");
    assert_eq!(
        step(&body, "work")["action"]["url"],
        format!("/thread/{thread}")
    );
    assert!(!step(&body, "work")["scripted"].as_bool().unwrap());
    let native = desk
        .world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&desk.ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(body, serde_json::to_value(native).unwrap());
}

#[tokio::test]
async fn first_steps_answer_keeps_the_scripted_identity_of_its_native_segment() {
    let desk = Desk::new().await;
    let thread = completed_answer(&desk, "fake").await;
    let token = key(
        &desk.world,
        desk.ada.id(),
        vec![
            rule("MeFirstStepsGet", "account", desk.ada.id()),
            home("MeFirstStepsGet", desk.org),
        ],
    )
    .await;
    let body = checked(
        desk.world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&FirstStepsGetQuery {
                org_id: Some(desk.org),
            })
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(step(&body, "work")["fact"], "answered_once");
    assert_eq!(
        step(&body, "work")["action"]["url"],
        format!("/thread/{thread}")
    );
    assert_eq!(step(&body, "work")["scripted"], true);
}

#[tokio::test]
async fn first_steps_publication_rechecks_the_native_enabled_model_after_the_actual_activity_wait()
{
    let world = World::new().await;
    let t = team(&world).await;
    orch_db::queries::resources::access_fixtures::insert_model(
        &mut *world.conn().await,
        t.provider,
        "first-wait-model",
        &["low"],
        true,
        t.boss.id(),
    )
    .await
    .unwrap();
    let query = FirstStepsGetQuery {
        org_id: Some(t.acme),
    };
    let token = key(
        &world,
        t.boss.id(),
        vec![
            rule("MeFirstStepsGet", "account", t.boss.id()),
            home("MeFirstStepsGet", t.acme),
        ],
    )
    .await;
    let control = checked(
        world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(step(&control, "models")["fact"], "models_enabled");
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app.call(MeFirstStepsGet, &[]).token(&token).query(&query);
        call.send_bytes().await
    });
    wait_for_activity(&world, pid, &request).await;
    sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1 AND model_name='first-wait-model'").bind(t.provider).execute(&world.db.pool).await.unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("models_enabled"));
}

#[tokio::test]
async fn first_steps_member_observation_uses_its_actual_domain_without_human_account_ancestry() {
    let world = World::new().await;
    let owner = places::person(&world, "first-members").await;
    let member = places::person(&world, "first-teammate").await;
    let org = places::make_org(&world, &owner, "First members", "first-members-team")
        .await
        .base
        .id;
    let domain = places::make_department(&world, &owner, org, "Actual joined domain", "joined")
        .await
        .base
        .id;
    places::join(&world, org, domain, &member, "viewer", &owner).await;
    let query = FirstStepsGetQuery { org_id: Some(org) };
    let token = key(
        &world,
        owner.id(),
        vec![
            rule("MeFirstStepsGet", "account", owner.id()),
            rule("MeFirstStepsGet", "org", org),
            rule("UserSearch", "account", member.id()),
        ],
    )
    .await;
    let body = checked(
        world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(step(&body, "teammate")["fact"], "unknown");
    let token = key(
        &world,
        owner.id(),
        vec![
            rule("MeFirstStepsGet", "account", owner.id()),
            home("MeFirstStepsGet", org),
        ],
    )
    .await;
    let body = checked(
        world
            .app
            .call(MeFirstStepsGet, &[])
            .token(&token)
            .query(&query)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(step(&body, "teammate")["fact"], "member_joined");
    let native = world
        .app
        .call(MeFirstStepsGet, &[])
        .token(&owner.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(body, serde_json::to_value(native).unwrap());
}
