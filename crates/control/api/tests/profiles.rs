//! A person's page grants no access, omits unseen work and records a member manager's private inspection.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;
mod usage_world;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::users::{
    MeProfilePinsSet, MeProfilePinsSetBody, MeProfileUpdate, MeProfileUpdateBody,
    ProfileActivityGet, ProfileAudience, ProfileGet, ProfileLink, ProfilePinInput, ProfilePinNode,
    UserProfile,
};
use orch_api_core::testing::Reply;
use orch_types::Id;
use places::{Who, person};
use support::{Statements, World};
use usage_world::{Use, another_thread, spend_in, team};

trait RefusalCheck {
    fn refused(self, code: ErrorCode) -> orch_api_contract::common::Problem;
}
impl<T: std::fmt::Debug> RefusalCheck for Reply<T> {
    fn refused(self, code: ErrorCode) -> orch_api_contract::common::Problem {
        let problem = self.problem();
        assert_eq!(problem.code, code);
        problem
    }
}

async fn page(world: &World, reader: &Who, username: &str) -> Reply<UserProfile> {
    world
        .app
        .call(ProfileGet, &[&username])
        .token(&reader.token)
        .send()
        .await
}

/// The page as one opening of it reads it: every read the screen makes carries the opening's identifier.
async fn opened(world: &World, reader: &Who, username: &str, opening: &str) -> Reply<UserProfile> {
    world
        .app
        .call(ProfileGet, &[&username])
        .token(&reader.token)
        .header("x-orch-opening", opening)
        .send()
        .await
}

async fn change(world: &World, person: &Who, body: MeProfileUpdateBody) -> Reply<UserProfile> {
    world
        .app
        .call(MeProfileUpdate, &[])
        .token(&person.token)
        .body(&body)
        .send()
        .await
}

async fn pin(world: &World, person: &Who, ids: &[Id]) -> Reply<UserProfile> {
    let body = MeProfilePinsSetBody {
        pins: ids
            .iter()
            .map(|id| ProfilePinInput::Node(ProfilePinNode { node_id: *id }))
            .collect(),
    };
    world
        .app
        .call(MeProfilePinsSet, &[])
        .token(&person.token)
        .body(&body)
        .send()
        .await
}

#[tokio::test]
async fn a_reader_gets_only_the_pins_and_domains_they_may_view_with_no_hidden_count() {
    let world = World::new().await;
    let t = team(&world).await;
    let personal = t.boss.person.personal_org;
    let own = pin(&world, &t.boss, &[t.acme, personal]).await.ok();
    assert_eq!(own.pins.len(), 2);
    assert!(own.settings.is_some());
    let username = t.boss.person.username.as_str();
    let peer = page(&world, &t.viewer, username).await.ok();
    assert_eq!(peer.pins.len(), 1);
    assert_eq!(peer.member_of.len(), 1);
    assert!(peer.personal_org.is_none());
    assert!(peer.settings.is_none());
    let stranger = page(&world, &t.stranger, username).await.ok();
    assert!(stranger.pins.is_empty());
    assert!(stranger.member_of.is_empty());
    assert!(!stranger.can_message);
    assert_eq!(
        pin(&world, &t.stranger, &[t.acme])
            .await
            .refused(ErrorCode::RequestInvalid)
            .status,
        400
    );
}

#[tokio::test]
async fn closed_missing_and_service_pages_answer_alike_but_a_member_manager_leaves_one_trace_for_each_opening()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let username = t.viewer.person.username.as_str();
    change(
        &world,
        &t.viewer,
        MeProfileUpdateBody {
            audience: Some(ProfileAudience::Orgs),
            ..Default::default()
        },
    )
    .await
    .ok();
    page(&world, &t.stranger, username)
        .await
        .refused(ErrorCode::NotFound);
    page(&world, &t.boss, username).await.ok();
    change(
        &world,
        &t.viewer,
        MeProfileUpdateBody {
            audience: Some(ProfileAudience::Private),
            ..Default::default()
        },
    )
    .await
    .ok();
    page(&world, &t.stranger, username)
        .await
        .refused(ErrorCode::NotFound);
    page(&world, &t.stranger, "nobody-has-this-name")
        .await
        .refused(ErrorCode::NotFound);
    let service = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        orch_db::queries::identity::create_service_account(
            &mut conn,
            service,
            t.acme,
            "automation",
            "Automation",
            t.boss.id(),
            world.now(),
        )
        .await
        .unwrap();
    }
    page(&world, &t.stranger, "automation")
        .await
        .refused(ErrorCode::NotFound);
    // An admin of the org reads a closed profile as an admin, with As admin on: one opening of it
    // is one trace, however many reads it makes.
    places::admin_mode(&world, &t.admin, true).await;
    let opening = orch_types::new_id().to_string();
    for _ in 0..2 {
        let read = opened(&world, &t.admin, username, &opening).await.ok();
        assert_eq!(read.inspected.unwrap().id, t.acme);
        assert!(read.settings.is_none());
        assert!(read.pins.is_empty());
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='profile.inspected' AND actor_id=$1 AND object_id=$2")
        .bind(t.admin.id()).bind(t.viewer.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(count, 1);
    world.advance(chrono::Duration::days(1));
    page(&world, &t.admin, username).await.ok();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='profile.inspected' AND actor_id=$1 AND object_id=$2")
        .bind(t.admin.id()).bind(t.viewer.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(count, 2);
    let other = person(&world, "other-admin").await;
    places::make_org(&world, &other, "Other", "other").await;
    page(&world, &other, username)
        .await
        .refused(ErrorCode::NotFound);
}

#[tokio::test]
async fn the_person_edits_their_words_switches_and_https_links_without_changing_omitted_values() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let edited = change(
        &world,
        &ada,
        MeProfileUpdateBody {
            bio: Some("Builds **tools**.".to_owned()),
            pronouns: Some("she/her".to_owned()),
            links: Some(vec![ProfileLink {
                label: "Work".to_owned(),
                url: "https://example.org".to_owned(),
            }]),
            show_time: Some(false),
            show_activity: Some(false),
            audience: Some(ProfileAudience::Private),
        },
    )
    .await
    .ok();
    assert_eq!(edited.bio, "Builds **tools**.");
    assert!(edited.local_time.is_none());
    assert!(edited.activity, "the person always sees their own work");
    assert!(!edited.settings.unwrap().show_activity);
    let updated = change(
        &world,
        &ada,
        MeProfileUpdateBody {
            pronouns: Some(String::new()),
            ..Default::default()
        },
    )
    .await
    .ok();
    assert_eq!(updated.bio, edited.bio);
    assert_eq!(updated.links.len(), 1);
    for url in ["http://example.org", "javascript:alert(1)", "https:"] {
        change(
            &world,
            &ada,
            MeProfileUpdateBody {
                links: Some(vec![ProfileLink {
                    label: "Work".to_owned(),
                    url: url.to_owned(),
                }]),
                ..Default::default()
            },
        )
        .await
        .refused(ErrorCode::RequestInvalid);
    }
    change(
        &world,
        &ada,
        MeProfileUpdateBody {
            bio: Some("a".repeat(2001)),
            ..Default::default()
        },
    )
    .await
    .refused(ErrorCode::RequestInvalid);
    change(
        &world,
        &ada,
        MeProfileUpdateBody {
            pronouns: Some("a".repeat(41)),
            ..Default::default()
        },
    )
    .await
    .refused(ErrorCode::RequestInvalid);
    let link = ProfileLink {
        label: "Work".to_owned(),
        url: "https://example.org".to_owned(),
    };
    change(
        &world,
        &ada,
        MeProfileUpdateBody {
            links: Some(vec![link; 7]),
            ..Default::default()
        },
    )
    .await
    .refused(ErrorCode::RequestInvalid);
    pin(
        &world,
        &ada,
        &[ada.person.personal_org, ada.person.personal_org],
    )
    .await
    .refused(ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_private_threads_work_never_enters_another_readers_days_or_totals_and_work_can_be_hidden()
{
    let world = World::new().await;
    let t = team(&world).await;
    let project =
        places::make_project(&world, &t.boss, t.boss.person.personal_org, "Personal").await;
    let topic = places::make_topic(&world, &t.boss, project.base.id, "Private work").await;
    let private = orch_types::new_id();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut *world.conn().await,
        private,
        "thread",
        Some(topic.base.id),
        "Private",
        t.boss.id(),
    )
    .await
    .unwrap();
    spend_in(&world, &t, t.thread, &Use::of(&world, t.boss.id(), 100)).await;
    spend_in(&world, &t, private, &Use::of(&world, t.boss.id(), 100)).await;
    let username = t.boss.person.username.as_str();
    let own = world
        .app
        .call(ProfileActivityGet, &[&username])
        .token(&t.boss.token)
        .send()
        .await
        .ok();
    let peer = world
        .app
        .call(ProfileActivityGet, &[&username])
        .token(&t.viewer.token)
        .send()
        .await
        .ok();
    let stranger = world
        .app
        .call(ProfileActivityGet, &[&username])
        .token(&t.stranger.token)
        .send()
        .await
        .ok();
    assert_eq!(own.total, 2);
    assert_eq!(peer.total, 1);
    assert_eq!(stranger.total, 0);
    assert!(stranger.days.is_empty());
    assert!(stranger.recent.is_empty());
    change(
        &world,
        &t.boss,
        MeProfileUpdateBody {
            show_activity: Some(false),
            ..Default::default()
        },
    )
    .await
    .ok();
    world
        .app
        .call(ProfileActivityGet, &[&username])
        .token(&t.viewer.token)
        .send()
        .await
        .refused(ErrorCode::NotFound);
    assert_eq!(
        world
            .app
            .call(ProfileActivityGet, &[&username])
            .token(&t.boss.token)
            .send()
            .await
            .ok()
            .total,
        2
    );
}

#[tokio::test]
async fn a_year_in_five_hundred_threads_costs_the_same_statements_as_one_thread() {
    // This is the requested scale fixture: 500 threads across all 371 calendar days, created in
    // one database and read through the real HTTP/authorization path; no time threshold depends on load.
    const THREADS: i64 = 500;
    const DAYS: i64 = 53 * 7;
    let world = World::new().await;
    let t = team(&world).await;
    let username = t.boss.person.username.as_str();
    spend_in(&world, &t, t.thread, &Use::of(&world, t.boss.id(), 100)).await;
    let before = Statements::now();
    world
        .app
        .call(ProfileActivityGet, &[&username])
        .token(&t.boss.token)
        .send()
        .await
        .ok();
    let few = Statements::now() - before;
    let mut ids = vec![t.thread];
    for index in 1..THREADS {
        ids.push(another_thread(&world, &t, &format!("Work {index}")).await);
    }
    let from = world.now().date_naive() - chrono::Duration::days(DAYS - 1);
    sqlx::query("DELETE FROM usage_daily WHERE user_id=$1")
        .bind(t.boss.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO usage_daily (day,org_id,project_id,topic_id,thread_id,provider_id,user_id,model,turns)
        SELECT $1::date + d::int,$2,$3,$4,t,$5,$6,'model',1 FROM generate_series(0,$7::bigint-1) d CROSS JOIN unnest($8::uuid[]) t")
        .bind(from).bind(t.acme).bind(t.project).bind(t.topic).bind(t.provider).bind(t.boss.id()).bind(DAYS).bind(&ids)
        .execute(&world.db.pool).await.unwrap();
    let before = Statements::now();
    let started = std::time::Instant::now();
    let answer = world
        .app
        .call(ProfileActivityGet, &[&username])
        .token(&t.boss.token)
        .send()
        .await
        .ok();
    let elapsed = started.elapsed();
    let many = Statements::now() - before;
    assert_eq!(answer.total, THREADS * DAYS);
    assert_eq!(i64::try_from(answer.days.len()).unwrap(), DAYS);
    assert_eq!(few, many);
    eprintln!(
        "profile activity: {THREADS} threads, {DAYS} days, {many} statements, {} ms",
        elapsed.as_millis()
    );
}
