//! Caps over HTTP: made, changed and removed by whoever manages the place they are defined on, read with how they stand, listed as everything that bounds work at a node, as a person's own share, and as the queue for room; a cap on a place covers what is inside it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;
mod usage_world;

use chrono::Duration;
use http::StatusCode;
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::usage::{
    CapAtLimit, CapInput, CapMeter, CapPeriod, CapSplit, CapState, CapStatus, CapSubject,
    CapSubjectInput, CapUncounted, MeLimitsGet, UsageCap, UsageCapCreate, UsageCapDelete,
    UsageCapList, UsageCapListQuery, UsageCapUpdate, UsageCapUpdateBody, UsageCapWaiting,
    UsageCapWaitingList, UsageLimitsGet, UsageLimitsGetQuery,
};
use orch_api_core::testing::Reply;
use orch_api_core::thread_access::AuthzAccess;
use orch_db::queries::access;
use orch_engine::access::{Limit, ResourceAccess};
use orch_types::{Id, new_id};
use places::{Who, make_org, missing_permissions, person};
use support::World;
use usage_world::{Team, Use, another_thread, spend, team};

#[tokio::test]
async fn period_caps_project_their_own_count_but_total_and_running_caps_have_no_forecast() {
    let world = World::new().await;
    let t = team(&world).await;
    let midnight = world
        .now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let enough = midnight + Duration::hours(6);
    if world.now() < enough {
        world.advance(enough - world.now());
    }
    let day = create(&world, &t.boss, &cap_input(Some(t.project)))
        .await
        .ok();
    let total = create(
        &world,
        &t.boss,
        &CapInput {
            period: Some(CapPeriod::Total),
            ..cap_input(Some(t.project))
        },
    )
    .await
    .ok();
    let running = create(&world, &t.boss, &running(Some(t.project), "2"))
        .await
        .ok();
    let rows = world
        .app
        .call(UsageCapList, &[])
        .token(&t.boss.token)
        .query(&UsageCapListQuery {
            node_id: Some(t.project),
            provider_id: None,
            system: None,
            covering: None,
        })
        .send()
        .await
        .ok();
    let projected = rows
        .iter()
        .find(|row| row.cap.id == day.id)
        .unwrap()
        .forecast
        .as_ref()
        .unwrap();
    assert_eq!(
        projected.basis,
        orch_provider_contract::forecast::ForecastBasis::Period
    );
    assert_eq!(projected.percent_at_end, 0.0);
    for id in [total.id, running.id] {
        assert!(
            rows.iter()
                .find(|row| row.cap.id == id)
                .unwrap()
                .forecast
                .is_none()
        );
    }
}

/// A cap on tokens a day that covers everybody together, defined on `node`.
fn cap_input(node: Option<Id>) -> CapInput {
    CapInput {
        node_id: node,
        provider_id: None,
        subject: CapSubjectInput::Everyone,
        split: CapSplit::Shared,
        models: None,
        tiers: None,
        harness_kinds: None,
        meter: CapMeter::Tokens,
        period: Some(CapPeriod::Day),
        counted_from: None,
        amount: Some("1000".to_owned()),
        at_limit: None,
        warn_percent: None,
        uncounted: None,
        note: None,
    }
}

/// A cap on turns running at once that covers everybody together.
fn running(node: Option<Id>, amount: &str) -> CapInput {
    CapInput {
        meter: CapMeter::Running,
        period: None,
        amount: Some(amount.to_owned()),
        ..cap_input(node)
    }
}

async fn create(world: &World, by: &Who, input: &CapInput) -> Reply<UsageCap> {
    world
        .app
        .call(UsageCapCreate, &[])
        .token(&by.token)
        .body(input)
        .send()
        .await
}

async fn made(world: &World, by: &Who, input: &CapInput) -> UsageCap {
    create(world, by, input).await.ok()
}

fn update_body(revision: i64) -> UsageCapUpdateBody {
    UsageCapUpdateBody {
        models: None,
        tiers: None,
        harness_kinds: None,
        counted_from: None,
        amount: None,
        at_limit: None,
        warn_percent: None,
        uncounted: None,
        note: None,
        enabled: None,
        expected_revision: revision,
    }
}

async fn change(world: &World, by: &Who, cap: Id, body: &UsageCapUpdateBody) -> Reply<UsageCap> {
    world
        .app
        .call(UsageCapUpdate, &[&cap])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

fn list_query(node: Option<Id>, provider: Option<Id>, system: bool) -> UsageCapListQuery {
    UsageCapListQuery {
        node_id: node,
        provider_id: provider,
        system: system.then_some(true),
        covering: None,
    }
}

async fn defined_at(world: &World, by: &Who, node: Id) -> Vec<CapStatus> {
    world
        .app
        .call(UsageCapList, &[])
        .token(&by.token)
        .query(&list_query(Some(node), None, false))
        .send()
        .await
        .ok()
}

async fn limits_at(
    world: &World,
    by: &Who,
    node: Id,
    user: Option<Id>,
) -> Reply<orch_api_contract::usage::Limits> {
    world
        .app
        .call(UsageLimitsGet, &[])
        .token(&by.token)
        .query(&UsageLimitsGetQuery {
            node_id: node,
            user_id: user,
        })
        .send()
        .await
}

async fn mine(world: &World, by: &Who) -> Vec<CapStatus> {
    world
        .app
        .call(MeLimitsGet, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
}

async fn audited(world: &World, action: &str) -> Vec<(String, bool)> {
    sqlx::query_as("SELECT actor_kind, history FROM audit_log WHERE action = $1 ORDER BY at, id")
        .bind(action)
        .fetch_all(&world.db.pool)
        .await
        .unwrap()
}

/// What the first fault of a refusal is about.
fn path_of(problem: &Problem) -> String {
    assert_eq!(
        problem.code,
        ErrorCode::RequestInvalid,
        "{}",
        problem.detail
    );
    problem.extra["errors"][0]["path"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn a_cap_is_made_by_whoever_manages_the_place_it_is_defined_on_and_refused_to_everybody_else()
{
    let world = World::new().await;
    let t = team(&world).await;
    let mut input = cap_input(Some(t.project));
    input.amount = Some("1000000".to_owned());
    input.warn_percent = Some(80);
    input.note = Some("for the quarter".to_owned());

    let reply = create(&world, &t.admin, &input).await;

    assert_eq!(reply.status, StatusCode::CREATED);
    let cap = reply.ok();
    assert_eq!(cap.node.as_ref().unwrap().name.as_deref(), Some("Billing"));
    assert!(cap.provider.is_none());
    assert_eq!(cap.subject, CapSubject::Everyone);
    assert_eq!(cap.amount.as_deref(), Some("1000000"));
    assert_eq!(
        (cap.at_limit, cap.uncounted),
        (CapAtLimit::Stop, CapUncounted::Allow)
    );
    assert_eq!(cap.warn_percent, Some(80));
    assert_eq!(cap.note, "for the quarter");
    assert_eq!((cap.revision, cap.enabled, cap.can_edit), (1, true, true));
    assert_eq!(cap.created_by.username, "admin");
    assert_eq!(
        audited(&world, "usage_cap.created").await,
        [("user".to_owned(), true)]
    );

    let mut other = cap_input(Some(t.project));
    other.amount = Some("5".to_owned());
    for (by, who) in [(&t.viewer, "viewer"), (&t.stranger, "stranger")] {
        let refused = create(&world, by, &other).await.problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{who}");
    }
    let refused = create(&world, &t.viewer, &other).await.problem();
    assert_eq!(missing_permissions(&refused), ["usage.manage"]);
    let nobody = world
        .app
        .call(UsageCapCreate, &[])
        .body(&other)
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    assert_eq!(
        defined_at(&world, &t.admin, t.project).await.len(),
        1,
        "none of the refused made one"
    );
}

#[tokio::test]
async fn a_cap_of_a_provider_is_made_by_whoever_manages_the_provider_and_one_of_the_system_by_a_system_admin()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let mut on_provider = cap_input(None);
    on_provider.provider_id = Some(t.provider);
    on_provider.meter = CapMeter::Credits;
    on_provider.amount = Some("50.5".to_owned());

    let refused = create(&world, &t.viewer, &on_provider).await.problem();
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    let cap = made(&world, &t.boss, &on_provider).await;
    assert_eq!(cap.provider.as_ref().unwrap().name, "Team key");
    assert!(cap.node.is_none());
    assert_eq!(cap.amount.as_deref(), Some("50.5"));

    let mut naming_a_node = on_provider.clone();
    naming_a_node.node_id = Some(t.project);
    naming_a_node.amount = Some("20".to_owned());
    let both = made(&world, &t.boss, &naming_a_node).await;
    assert!(
        both.node.is_some() && both.provider.is_some(),
        "a provider's cap may name a node as well"
    );
    let from_the_stranger = create(&world, &t.stranger, &naming_a_node).await.problem();
    assert_eq!(from_the_stranger.code, ErrorCode::AuthzDenied);

    let system = cap_input(None);
    let refused = create(&world, &t.boss, &system).await.problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "the owner of an org is no system admin"
    );
    let cap = made(&world, &t.sysadmin, &system).await;
    assert!(
        cap.node.is_none() && cap.provider.is_none(),
        "both null is the whole system"
    );
}

#[tokio::test]
async fn a_cap_that_exists_already_is_a_conflict_and_so_is_a_change_that_would_make_two_of_one() {
    let world = World::new().await;
    let t = team(&world).await;
    let mut family = cap_input(Some(t.project));
    family.models = Some(vec!["GPT-6*".to_owned()]);
    let first = made(&world, &t.admin, &family).await;
    assert_eq!(
        first.models,
        ["gpt-6*"],
        "models are kept in lower case, as they are compared"
    );

    let mut again = cap_input(Some(t.project));
    again.models = Some(vec!["  gpt-6*  ".to_owned(), "gpt-6*".to_owned()]);
    again.amount = Some("5".to_owned());
    let conflict = create(&world, &t.admin, &again).await.problem();
    assert_eq!(
        conflict.code,
        ErrorCode::ConflictExists,
        "the amount is not part of what a cap is: space, case and repeats do not make another"
    );

    let all_models = made(&world, &t.admin, &cap_input(Some(t.project))).await;
    let mut body = update_body(all_models.revision);
    body.models = Some(vec!["gpt-6*".to_owned()]);
    let clash = change(&world, &t.admin, all_models.id, &body)
        .await
        .problem();
    assert_eq!(clash.code, ErrorCode::ConflictExists);

    let mut each = cap_input(Some(t.project));
    each.split = CapSplit::Each;
    made(&world, &t.admin, &each).await;
    let mut per_kind = cap_input(Some(t.project));
    per_kind.harness_kinds = Some(vec!["codex".to_owned()]);
    made(&world, &t.admin, &per_kind).await;
}

/// Makes the cap that `broken` changes from a good one, and says what the refusal is about.
async fn refused_at(world: &World, t: &Team, broken: impl FnOnce(&mut CapInput)) -> String {
    let mut input = cap_input(Some(t.project));
    broken(&mut input);
    path_of(&create(world, &t.admin, &input).await.problem())
}

#[tokio::test]
async fn a_cap_is_checked_whole_before_it_is_made() {
    let world = World::new().await;
    let t = team(&world).await;
    // A cap on turns running at once has no period, warning, at_limit or uncounted.
    assert_eq!(
        refused_at(&world, &t, |i| *i = CapInput {
            period: Some(CapPeriod::Day),
            ..running(Some(t.project), "2")
        })
        .await,
        "period"
    );
    assert_eq!(
        refused_at(&world, &t, |i| *i = CapInput {
            warn_percent: Some(50),
            ..running(Some(t.project), "2")
        })
        .await,
        "warn_percent"
    );
    assert_eq!(
        refused_at(&world, &t, |i| *i = CapInput {
            at_limit: Some(CapAtLimit::Stop),
            ..running(Some(t.project), "2")
        })
        .await,
        "at_limit"
    );
    assert_eq!(
        refused_at(&world, &t, |i| *i = CapInput {
            uncounted: Some(CapUncounted::Block),
            ..running(Some(t.project), "2")
        })
        .await,
        "uncounted"
    );
    assert_eq!(
        refused_at(&world, &t, |i| *i = running(Some(t.project), "1.5")).await,
        "amount"
    );
    // A period is needed for what is counted over time.
    assert_eq!(refused_at(&world, &t, |i| i.period = None).await, "period");
    // Amounts: whole tokens, credits to six places, only digits.
    assert_eq!(
        refused_at(&world, &t, |i| i.amount = Some("1.5".to_owned())).await,
        "amount"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.amount = Some("ten".to_owned())).await,
        "amount"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.amount = Some("-5".to_owned())).await,
        "amount"
    );
    assert_eq!(
        refused_at(&world, &t, |i| {
            i.meter = CapMeter::Credits;
            i.amount = Some("0.1234567".to_owned());
        })
        .await,
        "amount"
    );
    // Warning percents are from 1 to 99.
    assert_eq!(
        refused_at(&world, &t, |i| i.warn_percent = Some(0)).await,
        "warn_percent"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.warn_percent = Some(100)).await,
        "warn_percent"
    );
    // The day counting begins on is for a total, and is a day.
    assert_eq!(
        refused_at(&world, &t, |i| i.counted_from =
            Some("2026-10-01".to_owned()))
        .await,
        "counted_from"
    );
    assert_eq!(
        refused_at(&world, &t, |i| {
            i.period = Some(CapPeriod::Total);
            i.counted_from = Some("last week".to_owned());
        })
        .await,
        "counted_from"
    );
    // Whom it covers must fit how it is counted.
    assert_eq!(
        refused_at(&world, &t, |i| {
            i.subject = CapSubjectInput::User { id: t.admin.id() };
            i.split = CapSplit::Each;
        })
        .await,
        "split"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.subject = CapSubjectInput::Role {
            role: orch_api_contract::access::Role::Developer,
            at: None,
        })
        .await,
        "subject"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.subject =
            CapSubjectInput::User { id: new_id() })
        .await,
        "subject.id"
    );
    // Models and harness kinds are names; an empty one would cover every model.
    assert_eq!(
        refused_at(&world, &t, |i| i.models = Some(vec![String::new()])).await,
        "models"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.models =
            Some(vec!["two words".to_owned()]))
        .await,
        "models"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.models =
            Some((0..51).map(|n| format!("m{n}")).collect()))
        .await,
        "models"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.harness_kinds = Some(vec![" ".to_owned()])).await,
        "harness_kinds"
    );
    assert_eq!(
        refused_at(&world, &t, |i| i.note = Some("x".repeat(501))).await,
        "note"
    );
    assert_eq!(
        defined_at(&world, &t.admin, t.project).await.len(),
        0,
        "nothing was written by any of them"
    );
}

#[tokio::test]
async fn a_group_is_the_subject_of_a_cap_only_where_its_maker_may_see_it() {
    let world = World::new().await;
    let t = team(&world).await;
    let outsider = person(&world, "outsider").await;
    let other_org = make_org(&world, &outsider, "Other", "other").await.base.id;
    let (own, foreign) = (new_id(), new_id());
    {
        let mut conn = world.conn().await;
        access::create_group(
            &mut conn,
            own,
            t.acme,
            "Backend",
            "",
            t.boss.id(),
            world.now(),
        )
        .await
        .unwrap();
        access::create_group(
            &mut conn,
            foreign,
            other_org,
            "Their backend",
            "",
            outsider.id(),
            world.now(),
        )
        .await
        .unwrap();
    }
    let mut input = cap_input(Some(t.project));
    input.subject = CapSubjectInput::Group { id: own };
    let cap = made(&world, &t.admin, &input).await;
    match cap.subject {
        CapSubject::Group { group } => {
            assert_eq!((group.id, group.name.as_str()), (own, "Backend"))
        }
        other => panic!("a group, not {other:?}"),
    }

    // A cap names the holders a page does: a group whose org its maker may see, and no other.
    input.subject = CapSubjectInput::Group { id: foreign };
    let unseen = create(&world, &t.admin, &input).await.problem();
    assert_eq!(unseen.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&unseen), ["org.view"]);
    input.subject = CapSubjectInput::Group { id: new_id() };
    assert_eq!(
        path_of(&create(&world, &t.admin, &input).await.problem()),
        "subject.id"
    );
    // The same for a cap of a provider. A system admin, acting as one, sees every org, and so may
    // name that group in a cap of the system.
    let mut provider = cap_input(None);
    provider.provider_id = Some(t.provider);
    provider.subject = CapSubjectInput::Group { id: own };
    made(&world, &t.boss, &provider).await;
    provider.subject = CapSubjectInput::Group { id: foreign };
    assert_eq!(
        create(&world, &t.boss, &provider).await.problem().code,
        ErrorCode::AuthzDenied
    );
    let mut system = cap_input(None);
    system.subject = CapSubjectInput::Group { id: foreign };
    made(&world, &t.sysadmin, &system).await;
    assert_eq!(
        sqlx::query_scalar::<_, Id>("SELECT created_by FROM usage_caps WHERE subject_id = $1")
            .bind(foreign)
            .fetch_all(&world.db.pool)
            .await
            .unwrap(),
        [t.sysadmin.id()],
        "no cap names a group its maker may not see"
    );
}

#[tokio::test]
async fn the_caps_of_a_place_are_read_with_how_they_stand_by_whoever_may_view_usage_there() {
    let world = World::new().await;
    let t = team(&world).await;
    let mut tokens = cap_input(Some(t.project));
    tokens.warn_percent = Some(50);
    let tokens = made(&world, &t.admin, &tokens).await;
    let at_once = made(&world, &t.admin, &running(Some(t.project), "2")).await;
    let off = made(&world, &t.admin, &{
        let mut input = cap_input(Some(t.project));
        input.period = Some(CapPeriod::Week);
        input
    })
    .await;
    let mut body = update_body(off.revision);
    body.enabled = Some(false);
    change(&world, &t.admin, off.id, &body).await.ok();
    spend(&world, &t, &Use::of(&world, t.viewer.id(), 600)).await;

    let listed = defined_at(&world, &t.admin, t.project).await;

    assert_eq!(
        listed
            .iter()
            .map(|status| status.cap.id)
            .collect::<Vec<_>>(),
        [tokens.id, at_once.id, off.id],
        "in the order they were made, those that are off among them"
    );
    let first = &listed[0];
    assert_eq!(
        (first.used.as_str(), first.in_progress.as_str()),
        ("600", "0")
    );
    assert_eq!(first.state, CapState::Warning, "past half of 1000 tokens");
    assert_eq!(first.person, None, "a shared cap has one row");
    assert!(first.period_ends_at.is_some());
    assert!(
        first.can_bypass,
        "the admin holds the row where the cap is defined"
    );
    assert_eq!(listed[1].used, "0");
    assert_eq!(listed[1].state, CapState::Ok);
    assert_eq!(
        listed[1].period_ends_at, None,
        "none for a cap on turns at once"
    );
    assert!(!listed[2].cap.enabled);

    let refused = world
        .app
        .call(UsageCapList, &[])
        .token(&t.viewer.token)
        .query(&list_query(Some(t.project), None, false))
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["usage.view"]);
    for bad in [
        list_query(None, None, false),
        list_query(Some(t.project), Some(t.provider), false),
        list_query(Some(t.project), None, true),
    ] {
        let invalid = world
            .app
            .call(UsageCapList, &[])
            .token(&t.admin.token)
            .query(&bad)
            .send()
            .await
            .problem();
        assert_eq!(
            invalid.code,
            ErrorCode::RequestInvalid,
            "exactly one of node_id, provider_id and system"
        );
    }
    let by_provider = world
        .app
        .call(UsageCapList, &[])
        .token(&t.viewer.token)
        .query(&list_query(None, Some(t.provider), false))
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&by_provider), ["provider.view"]);
    let system = world
        .app
        .call(UsageCapList, &[])
        .token(&t.boss.token)
        .query(&list_query(None, None, true))
        .send()
        .await
        .problem();
    assert_eq!(
        system.code,
        ErrorCode::AuthzDenied,
        "the caps of the system are for a system admin"
    );
    let empty: Vec<CapStatus> = world
        .app
        .call(UsageCapList, &[])
        .token(&t.sysadmin.token)
        .query(&list_query(None, None, true))
        .send()
        .await
        .ok();
    assert!(empty.is_empty());
}

#[tokio::test]
async fn the_caps_that_cover_work_at_a_node_are_listed_with_those_of_the_place_when_asked() {
    let world = World::new().await;
    let t = team(&world).await;
    let here = made(&world, &t.admin, &cap_input(Some(t.project))).await;
    let org = made(&world, &t.boss, &{
        let mut input = cap_input(Some(t.acme));
        input.amount = Some("9000".to_owned());
        input
    })
    .await;
    let system = made(&world, &t.sysadmin, &running(None, "50")).await;
    let provider = made(&world, &t.boss, &{
        let mut input = cap_input(None);
        input.provider_id = Some(t.provider);
        input.amount = Some("77".to_owned());
        input
    })
    .await;
    let mut query = list_query(Some(t.project), None, false);
    query.covering = Some(true);

    let listed: Vec<CapStatus> = world
        .app
        .call(UsageCapList, &[])
        .token(&t.admin.token)
        .query(&query)
        .send()
        .await
        .ok();

    let ids: Vec<Id> = listed.iter().map(|status| status.cap.id).collect();
    assert_eq!(ids[0], here.id, "the caps defined here come first");
    for covering in [org.id, system.id, provider.id] {
        assert!(
            ids.contains(&covering),
            "{covering} covers work in the project"
        );
    }
    assert_eq!(ids.len(), 4);
    let mut not_a_node = list_query(None, Some(t.provider), false);
    not_a_node.covering = Some(true);
    let invalid = world
        .app
        .call(UsageCapList, &[])
        .token(&t.boss.token)
        .query(&not_a_node)
        .send()
        .await
        .problem();
    assert_eq!(
        invalid.code,
        ErrorCode::RequestInvalid,
        "covering is asked of a node"
    );
}

#[tokio::test]
async fn a_cap_counted_per_person_shows_a_row_for_each_person_who_used_any_of_it_and_the_cap_that_applies_instead()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let mut each = cap_input(Some(t.project));
    each.split = CapSplit::Each;
    let each = made(&world, &t.admin, &each).await;
    let nobody = defined_at(&world, &t.admin, t.project).await;
    assert_eq!(
        nobody.len(),
        1,
        "a single row when nobody has used any of it"
    );
    assert!(nobody[0].person.is_none());
    assert_eq!(nobody[0].used, "0");

    spend(&world, &t, &Use::of(&world, t.viewer.id(), 600)).await;
    spend(&world, &t, &Use::of(&world, t.viewer.id(), 100)).await;
    spend(&world, &t, &Use::of(&world, t.admin.id(), 50)).await;
    let rows = defined_at(&world, &t.admin, t.project).await;
    let mut people: Vec<(String, String)> = rows
        .iter()
        .map(|row| (row.person.clone().unwrap().username, row.used.clone()))
        .collect();
    people.sort();
    assert_eq!(
        people,
        [
            ("admin".to_owned(), "50".to_owned()),
            ("viewer".to_owned(), "700".to_owned())
        ],
        "one row for each person, with their own share"
    );

    // The viewer is named in a cap of their own, which applies to them instead.
    let mut named = cap_input(Some(t.project));
    named.subject = CapSubjectInput::User { id: t.viewer.id() };
    named.amount = Some("5000".to_owned());
    let named = made(&world, &t.admin, &named).await;
    let rows = defined_at(&world, &t.admin, t.project).await;
    let of_each: Vec<&CapStatus> = rows.iter().filter(|row| row.cap.id == each.id).collect();
    let viewer_row = of_each
        .iter()
        .find(|row| row.person.as_ref().unwrap().username == "viewer")
        .unwrap();
    let admin_row = of_each
        .iter()
        .find(|row| row.person.as_ref().unwrap().username == "admin")
        .unwrap();
    assert_eq!(
        viewer_row.replaced_by,
        Some(named.id),
        "another cap applies to this person instead"
    );
    assert_eq!(admin_row.replaced_by, None);
}

#[tokio::test]
async fn a_cap_is_changed_against_the_revision_the_caller_saw() {
    let world = World::new().await;
    let t = team(&world).await;
    let cap = made(&world, &t.admin, &cap_input(Some(t.project))).await;

    let mut body = update_body(cap.revision);
    body.amount = Some(Some("2000".to_owned()));
    body.warn_percent = Some(Some(90));
    body.at_limit = Some(CapAtLimit::Stop);
    body.note = Some("raised".to_owned());
    let raised = change(&world, &t.admin, cap.id, &body).await.ok();
    assert_eq!(
        (raised.revision, raised.amount.as_deref()),
        (2, Some("2000"))
    );
    assert_eq!(
        (raised.warn_percent, raised.at_limit, raised.note.as_str()),
        (Some(90), CapAtLimit::Stop, "raised")
    );
    assert!(raised.updated_at.get() >= cap.updated_at.get());

    let stale = change(&world, &t.admin, cap.id, &body).await.problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        stale.extra["current"]["revision"], 2,
        "what it is now is told"
    );
    assert_eq!(stale.extra["current"]["amount"], "2000");

    let mut exempt = update_body(2);
    exempt.amount = Some(None);
    exempt.warn_percent = Some(None);
    let free = change(&world, &t.admin, cap.id, &exempt).await.ok();
    assert_eq!(
        (free.amount, free.warn_percent),
        (None, None),
        "null is no limit, not an amount left alone"
    );
    let mut off = update_body(3);
    off.enabled = Some(false);
    assert!(!change(&world, &t.admin, cap.id, &off).await.ok().enabled);
    assert_eq!(audited(&world, "usage_cap.updated").await.len(), 3);

    let mut by_viewer = update_body(4);
    by_viewer.note = Some("mine".to_owned());
    let refused = change(&world, &t.viewer, cap.id, &by_viewer)
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["usage.manage"]);
    let missing = change(&world, &t.admin, new_id(), &by_viewer)
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
    let nothing = change(&world, &t.admin, cap.id, &update_body(4))
        .await
        .problem();
    assert_eq!(
        nothing.code,
        ErrorCode::RequestInvalid,
        "a change names something to change"
    );

    let mut counted_from = update_body(4);
    counted_from.counted_from = Some(Some("2026-10-01".to_owned()));
    assert_eq!(
        path_of(
            &change(&world, &t.admin, cap.id, &counted_from)
                .await
                .problem()
        ),
        "counted_from"
    );
    let mut fraction = update_body(4);
    fraction.amount = Some(Some("1.5".to_owned()));
    assert_eq!(
        path_of(&change(&world, &t.admin, cap.id, &fraction).await.problem()),
        "amount"
    );
    let concurrent = made(&world, &t.admin, &running(Some(t.project), "2")).await;
    let mut at_limit = update_body(concurrent.revision);
    at_limit.at_limit = Some(CapAtLimit::Stop);
    assert_eq!(
        path_of(
            &change(&world, &t.admin, concurrent.id, &at_limit)
                .await
                .problem()
        ),
        "at_limit"
    );
    let mut fewer = update_body(concurrent.revision);
    fewer.amount = Some(Some("1".to_owned()));
    assert_eq!(
        change(&world, &t.admin, concurrent.id, &fewer)
            .await
            .ok()
            .amount
            .as_deref(),
        Some("1")
    );
}

#[tokio::test]
async fn a_cap_is_removed_by_whoever_manages_its_place_and_the_three_acts_are_in_the_audit_trail() {
    let world = World::new().await;
    let t = team(&world).await;
    let cap = made(&world, &t.admin, &cap_input(Some(t.project))).await;
    let mut body = update_body(1);
    body.note = Some("x".to_owned());
    change(&world, &t.admin, cap.id, &body).await.ok();

    let refused = world
        .app
        .call(UsageCapDelete, &[&cap.id])
        .token(&t.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["usage.manage"]);
    let gone = world
        .app
        .call(UsageCapDelete, &[&cap.id])
        .token(&t.admin.token)
        .send()
        .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    gone.ok();

    assert!(defined_at(&world, &t.admin, t.project).await.is_empty());
    let again = world
        .app
        .call(UsageCapDelete, &[&cap.id])
        .token(&t.admin.token)
        .send()
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::NotFound);
    for action in [
        "usage_cap.created",
        "usage_cap.updated",
        "usage_cap.deleted",
    ] {
        assert_eq!(audited(&world, action).await.len(), 1, "{action}");
    }
    // The cap's identical twin can be made again.
    made(&world, &t.admin, &cap_input(Some(t.project))).await;
}

#[tokio::test]
async fn the_limits_at_a_node_are_everything_that_bounds_work_there_and_a_cap_on_a_place_covers_what_is_inside_it()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let org = made(&world, &t.boss, &{
        let mut input = cap_input(Some(t.acme));
        input.amount = Some("9000".to_owned());
        input
    })
    .await;
    let project = made(&world, &t.admin, &running(Some(t.project), "2")).await;
    let topic = made(&world, &t.admin, &{
        let mut input = cap_input(Some(t.topic));
        input.split = CapSplit::Each;
        input.amount = Some("100".to_owned());
        input
    })
    .await;
    let provider = made(&world, &t.boss, &{
        let mut input = cap_input(None);
        input.provider_id = Some(t.provider);
        input.meter = CapMeter::Credits;
        input.amount = Some("10".to_owned());
        input
    })
    .await;
    let system = made(&world, &t.sysadmin, &running(None, "10")).await;
    // Caps that do not bound work in the thread: another project's, and one that is off.
    let sibling = person(&world, "sibling-boss").await;
    let elsewhere = make_org(&world, &sibling, "Elsewhere", "elsewhere")
        .await
        .base
        .id;
    made(&world, &sibling, &running(Some(elsewhere), "1")).await;
    let off = made(&world, &t.admin, &{
        let mut input = cap_input(Some(t.project));
        input.period = Some(CapPeriod::Month);
        input
    })
    .await;
    let mut body = update_body(off.revision);
    body.enabled = Some(false);
    change(&world, &t.admin, off.id, &body).await.ok();
    spend(&world, &t, &Use::of(&world, t.viewer.id(), 60)).await;

    let seen = limits_at(&world, &t.viewer, t.thread, None).await.ok().caps;

    assert_eq!(
        seen.iter().map(|status| status.cap.id).collect::<Vec<_>>(),
        [org.id, project.id, topic.id, provider.id, system.id],
        "the org's, the project's, the topic's, the provider's and the system's: what lies above covers what is inside"
    );
    let on_topic = &seen[2];
    assert_eq!(
        on_topic.person.as_ref().unwrap().username,
        "viewer",
        "a cap counted per person shows the caller's share"
    );
    assert_eq!(
        (on_topic.used.as_str(), on_topic.state),
        ("60", CapState::Ok)
    );
    assert!(!on_topic.can_bypass, "a viewer holds no row to pass it");
    assert!(
        seen[0].person.is_none(),
        "a shared cap is the same for everybody"
    );
    assert_eq!(seen[0].used, "60");

    let by_admin = limits_at(&world, &t.admin, t.thread, None).await.ok().caps;
    assert!(by_admin.iter().all(|status| status.can_bypass
        || status.cap.id == provider.id
        || status.cap.id == system.id));
    assert!(by_admin[2].can_bypass);
    assert!(
        by_admin[2]
            .person
            .as_ref()
            .is_some_and(|person| person.username == "admin"),
        "their own share, not the viewer's"
    );
    assert_eq!(by_admin[2].used, "0");

    let refused = limits_at(&world, &t.stranger, t.thread, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let missing = limits_at(&world, &t.viewer, new_id(), None).await.problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn somebody_elses_share_of_the_limits_is_for_a_reader_of_usage_only() {
    let world = World::new().await;
    let t = team(&world).await;
    made(&world, &t.admin, &{
        let mut input = cap_input(Some(t.project));
        input.split = CapSplit::Each;
        input.amount = Some("100".to_owned());
        input
    })
    .await;
    spend(&world, &t, &Use::of(&world, t.viewer.id(), 80)).await;

    let theirs = limits_at(&world, &t.admin, t.project, Some(t.viewer.id()))
        .await
        .ok()
        .caps;
    assert_eq!(theirs[0].person.as_ref().unwrap().username, "viewer");
    assert_eq!(
        (theirs[0].used.as_str(), theirs[0].state),
        ("80", CapState::Ok)
    );
    let own = limits_at(&world, &t.viewer, t.project, Some(t.viewer.id()))
        .await
        .ok()
        .caps;
    assert_eq!(own[0].used, "80", "asking for oneself needs nothing more");
    let refused = limits_at(&world, &t.viewer, t.project, Some(t.admin.id()))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["usage.view"]);
}

#[tokio::test]
async fn a_person_reads_their_own_share_of_every_cap_counted_per_person_that_covers_them_anywhere()
{
    let world = World::new().await;
    let t = team(&world).await;
    let group = new_id();
    {
        let mut conn = world.conn().await;
        access::create_group(
            &mut conn,
            group,
            t.acme,
            "Backend",
            "",
            t.boss.id(),
            world.now(),
        )
        .await
        .unwrap();
        access::add_group_member(&mut conn, group, t.admin.id(), t.boss.id(), world.now())
            .await
            .unwrap();
    }
    let for_everyone = made(&world, &t.admin, &{
        let mut input = cap_input(Some(t.project));
        input.split = CapSplit::Each;
        input
    })
    .await;
    let for_the_group = made(&world, &t.admin, &{
        let mut input = cap_input(Some(t.project));
        input.split = CapSplit::Each;
        input.subject = CapSubjectInput::Group { id: group };
        input.amount = Some("5000".to_owned());
        input
    })
    .await;
    let shared = made(&world, &t.admin, &cap_input(Some(t.topic))).await;
    spend(&world, &t, &Use::of(&world, t.admin.id(), 300)).await;

    let viewer = mine(&world, &t.viewer).await;
    assert_eq!(
        viewer
            .iter()
            .map(|status| status.cap.id)
            .collect::<Vec<_>>(),
        [for_everyone.id],
        "the group's cap does not cover somebody outside it, and a shared cap is no one's share"
    );
    assert_eq!(viewer[0].person.as_ref().unwrap().username, "viewer");
    assert_eq!(viewer[0].used, "0");

    let admin = mine(&world, &t.admin).await;
    assert_eq!(
        admin.iter().map(|status| status.cap.id).collect::<Vec<_>>(),
        [for_everyone.id, for_the_group.id]
    );
    assert_eq!(
        admin[0].replaced_by,
        Some(for_the_group.id),
        "the largest cap of the group applies instead of everyone's"
    );
    assert_eq!(admin[1].replaced_by, None);
    assert_eq!(admin[1].used, "300");
    assert!(admin.iter().all(|status| status.cap.id != shared.id));
    let nobody = world.app.call(MeLimitsGet, &[]).send().await.problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

/// Puts a thread in the state of one that waits for room in a cap, with an input of `author` since `ago`.
/// It runs on the team's provider, with an agent and a model, as a thread a cap holds back does: what a
/// cap covers is what the thread's next turn would ask for.
async fn wait(world: &World, t: &Team, thread: Id, author: Id, ago: Duration) {
    let mut conn = world.conn().await;
    let agent = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "agent",
        Some(t.boss.id()),
        Some(t.acme),
        &format!("agent-{}", thread.simple()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET provider_id = $2, agent_id = $3 WHERE node_id = $1")
        .bind(thread)
        .bind(t.provider)
        .bind(agent)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO settings (node_id, key, value) VALUES ($1, 'agent.model', '\"gpt-test\"'::jsonb)",
    )
    .bind(thread)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE threads SET state = 'blocked', blocked_reason = 'quota_exceeded', blocked_detail = 'full' WHERE node_id = $1",
    )
    .bind(thread)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO thread_inputs (id, thread_id, author_id, source, content, delivery, state, position, seq_updated, created_at)
         VALUES ($1, $2, $3, 'user', '[]', 'queue', 'pending', 1, 1, $4)",
    )
    .bind(new_id())
    .bind(thread)
    .bind(author)
    .bind(world.now() - ago)
    .execute(&mut *conn)
    .await
    .unwrap();
}

async fn queue(
    world: &World,
    by: &Who,
    cap: Id,
) -> Reply<orch_api_contract::usage::UsageCapWaitingListReply> {
    world
        .app
        .call(UsageCapWaitingList, &[&cap])
        .token(&by.token)
        .send()
        .await
}

fn names_of(items: &[UsageCapWaiting]) -> Vec<(Option<String>, Option<String>, bool)> {
    items
        .iter()
        .map(|item| {
            (
                item.thread.as_ref().and_then(|thread| thread.name.clone()),
                item.author.as_ref().map(|author| author.username.clone()),
                item.mine,
            )
        })
        .collect()
}

#[tokio::test]
async fn what_waits_for_room_is_listed_in_the_order_it_will_run_and_a_covered_person_sees_only_their_own_rows_named()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let full = made(&world, &t.admin, &running(Some(t.project), "0")).await;
    let second = another_thread(&world, &t, "Second").await;
    wait(&world, &t, t.thread, t.viewer.id(), Duration::minutes(30)).await;
    wait(&world, &t, second, t.admin.id(), Duration::minutes(10)).await;

    let reader = queue(&world, &t.admin, full.id).await.ok().items;

    assert_eq!(
        names_of(&reader),
        [
            (
                Some("Fix totals".to_owned()),
                Some("viewer".to_owned()),
                false
            ),
            (Some("Second".to_owned()), Some("admin".to_owned()), true),
        ],
        "the one that has waited longest first; a reader of the cap sees them all named"
    );
    assert!(reader[0].since.get() < reader[1].since.get());

    let covered = queue(&world, &t.viewer, full.id).await.ok().items;
    assert_eq!(
        names_of(&covered),
        [
            (
                Some("Fix totals".to_owned()),
                Some("viewer".to_owned()),
                true
            ),
            (None, None, false),
        ],
        "somebody the cap only covers sees their own row named and the others as places in the queue"
    );

    let has_room = made(&world, &t.admin, &running(Some(t.topic), "5")).await;
    assert!(
        queue(&world, &t.admin, has_room.id)
            .await
            .ok()
            .items
            .is_empty(),
        "nobody waits for a cap that has room, even in a thread that another cap blocks"
    );
}

#[tokio::test]
async fn the_queue_of_a_cap_is_for_those_who_read_it_or_are_covered_by_it() {
    let world = World::new().await;
    let t = team(&world).await;
    let mut for_the_admin = running(Some(t.project), "0");
    for_the_admin.subject = CapSubjectInput::User { id: t.admin.id() };
    let cap = made(&world, &t.admin, &for_the_admin).await;
    wait(&world, &t, t.thread, t.admin.id(), Duration::minutes(5)).await;

    let covered = queue(&world, &t.admin, cap.id).await.ok().items;
    assert_eq!(covered.len(), 1);
    let refused = queue(&world, &t.viewer, cap.id).await.problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "neither a reader of the cap nor covered by it"
    );
    assert_eq!(missing_permissions(&refused), ["usage.view"]);
    let missing = queue(&world, &t.admin, new_id()).await.problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
    let nobody = world
        .app
        .call(UsageCapWaitingList, &[&cap.id])
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn whether_the_caller_may_let_a_turn_past_a_cap_is_said_with_the_cap() {
    let world = World::new().await;
    let t = team(&world).await;
    let node = made(&world, &t.admin, &cap_input(Some(t.project))).await;
    let provider = made(&world, &t.boss, &{
        let mut input = cap_input(None);
        input.provider_id = Some(t.provider);
        input.amount = Some("7".to_owned());
        input
    })
    .await;

    let admin = limits_at(&world, &t.admin, t.project, None).await.ok().caps;
    let by_id = |caps: &[CapStatus], id: Id| {
        caps.iter()
            .find(|status| status.cap.id == id)
            .unwrap()
            .can_bypass
    };
    assert!(
        by_id(&admin, node.id),
        "usage.bypass on the node, which usage.manage holds"
    );
    assert!(
        !by_id(&admin, provider.id),
        "the admin of the org is not a manager of the provider"
    );
    let boss = limits_at(&world, &t.boss, t.project, None).await.ok().caps;
    assert!(
        by_id(&boss, node.id) && by_id(&boss, provider.id),
        "an owner holds both"
    );
    let viewer = limits_at(&world, &t.viewer, t.project, None)
        .await
        .ok()
        .caps;
    assert!(!by_id(&viewer, node.id) && !by_id(&viewer, provider.id));
}

#[tokio::test]
async fn a_turn_is_let_past_a_cap_by_whoever_holds_the_row_where_the_cap_is_defined_and_nobody_else()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let on_the_project = made(&world, &t.admin, &cap_input(Some(t.project))).await;
    let on_the_provider = made(&world, &t.boss, &{
        let mut input = cap_input(None);
        input.provider_id = Some(t.provider);
        input
    })
    .await;
    let of_the_system = made(&world, &t.sysadmin, &cap_input(None)).await;
    let access = AuthzAccess::new(world.app.app.clock.clone());
    // Whether `who` may let a turn past `cap`, acting with As admin on or off.
    let may = |who: Id, as_admin: bool, cap: Id| {
        let (access, world) = (&access, &world);
        async move {
            let mut conn = world.conn().await;
            access
                .may_pass(&mut conn, who, as_admin, &Limit::Cap { cap })
                .await
                .unwrap()
        }
    };

    assert!(
        may(t.admin.id(), false, on_the_project.id).await,
        "usage.bypass on the node, which usage.manage holds"
    );
    assert!(
        may(t.boss.id(), false, on_the_project.id).await,
        "the owner of the org"
    );
    assert!(!may(t.viewer.id(), false, on_the_project.id).await);
    assert!(!may(t.stranger.id(), false, on_the_project.id).await);

    assert!(
        may(t.boss.id(), false, on_the_provider.id).await,
        "the owner of the provider"
    );
    assert!(
        !may(t.admin.id(), false, on_the_provider.id).await,
        "an admin of the org is not a manager of its provider"
    );
    assert!(!may(t.viewer.id(), false, on_the_provider.id).await);

    assert!(
        may(t.sysadmin.id(), true, of_the_system.id).await,
        "a system admin, for a cap of the system, acting as one"
    );
    assert!(
        !may(t.sysadmin.id(), false, of_the_system.id).await,
        "a system admin with As admin off acts as nobody's admin"
    );
    assert!(!may(t.boss.id(), false, of_the_system.id).await);

    assert!(
        !may(t.sysadmin.id(), true, new_id()).await,
        "a cap that is not there is passed by nobody"
    );
}

// ---------------------------------------------------------------- the models and the speeds a cap names

#[tokio::test]
async fn a_cap_names_a_group_of_a_providers_models_and_the_speeds_it_covers_and_is_the_same_cap_by_both()
 {
    let world = World::new().await;
    let t = team(&world).await;
    let mut input = cap_input(Some(t.project));
    input.models = Some(vec![" @Frontier ".to_owned(), "gpt-6*".to_owned()]);
    input.tiers = Some(vec!["fast".to_owned()]);
    let cap = made(&world, &t.admin, &input).await;
    assert_eq!(
        cap.models,
        ["@frontier", "gpt-6*"],
        "a group is written like every pattern: in lower case, once, in order"
    );
    assert_eq!(cap.tiers, ["fast"]);

    // The same node, provider, subject, split, models, tiers, harness kinds, meter and period is the same cap.
    let conflict = create(&world, &t.admin, &input).await.problem();
    assert_eq!(conflict.code, ErrorCode::ConflictExists);
    // Another speed, or none, is another cap.
    let mut other_tier = input.clone();
    other_tier.tiers = Some(vec!["flex".to_owned()]);
    made(&world, &t.admin, &other_tier).await;
    let mut no_tier = input.clone();
    no_tier.tiers = None;
    let all_speeds = made(&world, &t.admin, &no_tier).await;
    assert!(
        all_speeds.tiers.is_empty(),
        "no speed named: every speed, the ordinary one too"
    );

    // A change into the same as another is refused the same way, and one that is not is made.
    let mut body = update_body(all_speeds.revision);
    body.tiers = Some(vec!["fast".to_owned()]);
    let clash = change(&world, &t.admin, all_speeds.id, &body)
        .await
        .problem();
    assert_eq!(clash.code, ErrorCode::ConflictExists);
    body.tiers = Some(vec!["fast".to_owned(), "flex".to_owned()]);
    let changed = change(&world, &t.admin, all_speeds.id, &body).await.ok();
    assert_eq!(changed.tiers, ["fast", "flex"]);
    assert_eq!(changed.revision, all_speeds.revision + 1);
    // A list of speeds that is emptied takes the limit off the speed: it covers every speed again.
    let mut body = update_body(changed.revision);
    body.tiers = Some(Vec::new());
    let emptied = change(&world, &t.admin, changed.id, &body).await.ok();
    assert!(emptied.tiers.is_empty());

    // They are listed with the cap.
    let listed = defined_at(&world, &t.admin, t.project).await;
    let fast = listed
        .iter()
        .find(|status| status.cap.id == cap.id)
        .unwrap();
    assert_eq!(fast.cap.models, ["@frontier", "gpt-6*"]);
    assert_eq!(fast.cap.tiers, ["fast"]);
}

#[tokio::test]
async fn a_group_is_named_on_a_cap_as_a_provider_s_managers_may_name_one_and_nothing_else_is_taken_for_it()
 {
    let world = World::new().await;
    let t = team(&world).await;
    for bad in [
        "@",
        "@-x",
        "@a b",
        "@Fron/tier",
        &format!("@{}", "g".repeat(41)),
    ] {
        let mut input = cap_input(Some(t.project));
        input.models = Some(vec![bad.to_owned()]);
        let problem = create(&world, &t.admin, &input).await.problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid, "{bad:?}");
    }
    // A name that has an @ in the middle of it is a model's name, which is opaque.
    let mut input = cap_input(Some(t.project));
    input.models = Some(vec!["vendor@gpt".to_owned()]);
    made(&world, &t.admin, &input).await;
    // Speeds are words like harness kinds: none empty, none with a space.
    let mut input = cap_input(Some(t.project));
    input.tiers = Some(vec!["fast mode".to_owned()]);
    let problem = create(&world, &t.admin, &input).await.problem();
    assert_eq!(path_of(&problem), "tiers");
}
