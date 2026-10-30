//! Shells over HTTP: who may open one, the gates no permission lifts, the two limits and which shell a person gives up for one more, who is given a ticket, who closes a shell, what is kept of one in its environment's history, and the ticket of the agent's own terminal.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;
mod terminals_support;

use chrono::Duration;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::terminals::{
    AgentTerminalTicketCreate, AgentTerminalTicketCreateBody, TerminalRecordingGet, TerminalState,
};
use orch_api_core::registry::Registry;
use orch_settings::Target;
use serde_json::json;
use std::collections::HashSet;
use terminals_support::Floor;

/// A body that opens a shell of a size.
fn sized() -> serde_json::Value {
    json!({ "cols": 100, "rows": 30 })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_developer_opens_a_shell_in_the_environment_of_a_thread_and_is_given_a_ticket_for_it() {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    let made = floor.open(dan).await.ok();
    let session = &made.session;
    assert_eq!(session.state, TerminalState::Open);
    assert_eq!(session.opened_by.username, "dan");
    assert_eq!(session.thread_id, Some(floor.thread));
    assert_eq!(session.environment_id, floor.environment);
    assert_eq!(session.shell, "/bin/fakesh");
    assert_eq!(
        session.cwd, "/workspace/web",
        "from a thread it starts in the thread's own directory"
    );
    assert!(!session.attached, "nobody has it on a screen yet");
    assert!(
        session.recorded,
        "the last of what it prints is kept, as shipped"
    );
    assert!(
        session.closes_at.is_some(),
        "left alone, it is closed after a while"
    );
    assert!(!made.ticket.is_empty());
    assert_eq!(
        made.ticket_expires_at.get() - floor.scene.rig.world.now(),
        Duration::seconds(30),
        "a ticket is good for thirty seconds"
    );
    assert!(made.closed.is_empty());
    let asked = floor.fake.opened.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        (
            asked[0].cols,
            asked[0].rows,
            asked[0].cwd.as_str(),
            asked[0].record
        ),
        (100, 30, "web", true)
    );
    assert_eq!(asked[0].terminal_id, session.id);
    assert_eq!(
        floor.history().await,
        [(
            "terminal.opened".to_owned(),
            "dan opened a shell".to_owned()
        )]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn whoever_manages_the_environment_opens_a_shell_without_a_thread_in_the_root_or_in_a_directory_under_it()
 {
    let floor = Floor::new().await;
    let eve = &floor.scene.eve;
    let at_root = floor.open_with(eve, None, sized()).await.ok();
    assert_eq!(at_root.session.thread_id, None);
    assert_eq!(at_root.session.cwd, "/workspace");
    let below = floor
        .open_with(
            eve,
            None,
            json!({ "cols": 80, "rows": 24, "cwd": "web/src" }),
        )
        .await
        .ok();
    assert_eq!(below.session.cwd, "/workspace/web/src");
    let asked = floor.fake.opened.lock().unwrap().clone();
    assert_eq!(asked[1].cwd, "web/src");
    let outside = floor
        .open_with(
            eve,
            None,
            json!({ "cols": 80, "rows": 24, "cwd": "../etc" }),
        )
        .await
        .problem();
    assert_eq!(outside.code, ErrorCode::RequestInvalid);
}

#[tokio::test(flavor = "multi_thread")]
async fn somebody_who_holds_nothing_on_the_environment_is_refused_with_what_is_missing_there() {
    let floor = Floor::new().await;
    let viewer = floor.open(&floor.scene.cai).await.problem();
    assert_eq!(viewer.code, ErrorCode::AuthzDenied);
    assert!(
        viewer.extra["missing"]
            .to_string()
            .contains("terminal.manage"),
        "{viewer:?}"
    );
    // Everything on the thread is no way into the environment: a thread's page holds no shell, and
    // what is missing is named on the environment.
    floor
        .names_on_the_thread(&floor.scene.bea, "thread.manage")
        .await;
    let through_the_thread = floor.open(&floor.scene.bea).await.problem();
    assert_eq!(through_the_thread.code, ErrorCode::AuthzDenied);
    let missing = through_the_thread.extra["missing"].to_string();
    assert!(missing.contains("terminal.manage"), "{missing}");
    assert!(missing.contains("environment"), "{missing}");
    let from_the_page = floor
        .open_with(&floor.scene.bea, None, sized())
        .await
        .problem();
    assert_eq!(from_the_page.code, ErrorCode::AuthzDenied);
    assert_eq!(
        floor.fake.opened.lock().unwrap().len(),
        0,
        "the machine was asked for none"
    );
    // The row of the environment's page is what lets a person open one, beside the thread and from the
    // page alike.
    floor
        .names_on_the_environment(&floor.scene.bea, "terminal.manage")
        .await;
    floor.open(&floor.scene.bea).await.ok();
    floor.open_with(&floor.scene.bea, None, sized()).await.ok();
    assert_eq!(
        floor.fake.opened.lock().unwrap().len(),
        2,
        "the machine was asked for the two that were allowed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thread_that_does_not_run_in_the_environment_opens_nothing_in_it() {
    let floor = Floor::new().await;
    // Dan may open shells through the thread, in the environment it runs in and in no other.
    let elsewhere = floor
        .scene
        .rig
        .world
        .app
        .call(
            orch_api_contract::terminals::EnvironmentTerminalCreate,
            &[&orch_types::new_id()],
        )
        .token(&floor.scene.dan.token)
        .raw_query(&format!("thread={}", floor.thread))
        .raw_body(&sized())
        .send()
        .await
        .problem();
    assert_eq!(elsewhere.code, ErrorCode::NotFound);
}

/// A shell opened by `dan` in the usual way, as the tests below begin.
async fn refusal_of_the_next_shell(floor: &Floor) -> orch_api_contract::common::Problem {
    floor.open(&floor.scene.dan).await.problem()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_setting_that_forbids_shells_refuses_them_with_its_reason() {
    let floor = Floor::new().await;
    floor
        .scene
        .rig
        .world
        .set_at(
            Target::Node(floor.scene.tree.topic.base.id),
            "terminal.enabled",
            json!(false),
        )
        .await;
    let refused = refusal_of_the_next_shell(&floor).await;
    assert_eq!(refused.code, ErrorCode::TerminalNotAllowed);
    assert_eq!(refused.extra["reason"], "disabled_by_policy");
    // A setting is below no page: a manager of the environment is refused at the environment too.
    let manager = floor
        .open_with(&floor.scene.eve, None, sized())
        .await
        .problem();
    assert_eq!(manager.extra["reason"], "disabled_by_policy");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_whose_own_switch_is_off_refuses_shells_with_its_reason() {
    let floor = Floor::new().await;
    sqlx::query("UPDATE environments SET terminal = false WHERE id = $1")
        .bind(floor.environment)
        .execute(floor.pool())
        .await
        .unwrap();
    let refused = refusal_of_the_next_shell(&floor).await;
    assert_eq!(refused.code, ErrorCode::TerminalNotAllowed);
    assert_eq!(refused.extra["reason"], "environment_switch_off");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_persons_machine_that_accepts_no_shells_from_outside_refuses_them_with_its_reason() {
    let floor = Floor::new().await;
    let machine = floor.scene.rig.machine_id;
    sqlx::query(
        "UPDATE environments SET placement = 'connected', template_id = NULL WHERE id = $1",
    )
    .bind(floor.environment)
    .execute(floor.pool())
    .await
    .unwrap();
    // On a person's machine nobody but whoever its owner names manages anything: Eve is named.
    let eve = &floor.scene.eve;
    let refused = floor.open_with(eve, None, sized()).await.problem();
    assert_eq!(refused.code, ErrorCode::TerminalNotAllowed);
    assert_eq!(refused.extra["reason"], "machine_policy");
    // Its owner hands shells over, and they open.
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_terminal: true,
        ..orch_link_contract::policy::LocalPolicy::default()
    };
    sqlx::query(
        "UPDATE machines SET facts = jsonb_build_object('policy', $2::jsonb) WHERE id = $1",
    )
    .bind(machine)
    .bind(serde_json::to_value(&policy).unwrap())
    .execute(floor.pool())
    .await
    .unwrap();
    floor.open_with(eve, None, sized()).await.ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_that_cannot_be_reached_refuses_shells_with_its_reason() {
    let floor = Floor::new().await;
    sqlx::query("UPDATE environments SET status = 'stopped' WHERE id = $1")
        .bind(floor.environment)
        .execute(floor.pool())
        .await
        .unwrap();
    let stopped = refusal_of_the_next_shell(&floor).await;
    assert_eq!(stopped.code, ErrorCode::TerminalNotAllowed);
    assert_eq!(stopped.extra["reason"], "environment_offline");
    sqlx::query("UPDATE environments SET status = 'running' WHERE id = $1")
        .bind(floor.environment)
        .execute(floor.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online = false WHERE id = $1")
        .bind(floor.scene.rig.machine_id)
        .execute(floor.pool())
        .await
        .unwrap();
    let away = refusal_of_the_next_shell(&floor).await;
    assert_eq!(away.extra["reason"], "environment_offline");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_that_will_not_start_the_shell_leaves_nothing_behind() {
    let floor = Floor::new().await;
    *floor.fake.refuse.lock().unwrap() = Some(orch_link_contract::commands::CommandError::new(
        orch_link_contract::commands::ErrorCode::Internal,
        "no pty",
    ));
    let refused = floor.open(&floor.scene.dan).await.problem();
    assert_eq!(refused.code, ErrorCode::EnvironmentUnavailable);
    assert!(floor.list(&floor.scene.dan, None).await.is_empty());
    assert!(
        floor.history().await.is_empty(),
        "nothing happened that a history would say"
    );
    floor.open(&floor.scene.dan).await.ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn at_the_limit_a_person_is_shown_their_own_shells_and_gives_up_the_ones_they_choose_for_a_new_one()
 {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    let mut opened = Vec::new();
    for _ in 0..4 {
        opened.push(floor.shell_of(dan).await.session.id);
        floor.scene.rig.world.advance(Duration::seconds(1));
    }
    let at_the_limit = floor.open(dan).await.problem();
    assert_eq!(at_the_limit.code, ErrorCode::ConflictExists);
    assert_eq!(at_the_limit.extra["limit"], "person");
    let mine: Vec<String> = at_the_limit.extra["mine"]
        .as_array()
        .unwrap()
        .iter()
        .map(|each| each["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        mine,
        opened.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "their own, the one used longest ago first"
    );
    assert_eq!(
        floor.list(dan, Some("open")).await.len(),
        4,
        "nothing was closed"
    );
    // They give up the second and the third, and the new one opens as they close.
    let made = floor
        .open_with(
            dan,
            Some(floor.thread),
            json!({ "cols": 80, "rows": 24, "close": [opened[1], opened[2]] }),
        )
        .await
        .ok();
    assert_eq!(made.closed, [opened[1], opened[2]]);
    let still_open: HashSet<_> = floor
        .list(dan, Some("open"))
        .await
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(
        still_open,
        HashSet::from([opened[0], opened[3], made.session.id])
    );
    let closed = floor.list(dan, Some("closed")).await;
    assert_eq!(closed.len(), 2);
    assert!(
        closed
            .iter()
            .all(|session| session.close_reason.as_deref() == Some("closed_by_user"))
    );
    assert_eq!(
        floor.fake.closed.lock().unwrap().len(),
        2,
        "the machine closed them"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn giving_up_too_few_shells_closes_none_and_still_says_what_there_is() {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    let mut opened = Vec::new();
    for _ in 0..4 {
        opened.push(floor.shell_of(dan).await.session.id);
    }
    // Two of them are named, and the environment's limit is what stops the new one.
    floor
        .scene
        .rig
        .world
        .set_at(
            Target::Node(floor.scene.tree.topic.base.id),
            "terminal.max_per_environment",
            json!(3),
        )
        .await;
    let refused = floor
        .open_with(
            dan,
            Some(floor.thread),
            json!({ "cols": 80, "rows": 24, "close": [opened[0]] }),
        )
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictExists);
    assert_eq!(
        floor.list(dan, Some("open")).await.len(),
        4,
        "a request that cannot be served closes nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_environments_limit_counts_everybodys_shells_and_says_how_many_people_have_them() {
    let floor = Floor::new().await;
    floor
        .scene
        .rig
        .world
        .set_at(
            Target::Node(floor.scene.tree.topic.base.id),
            "terminal.max_per_environment",
            json!(2),
        )
        .await;
    let theirs = floor.shell_of(&floor.scene.dan).await.session.id;
    floor.shell_of(&floor.scene.eve).await;
    // Ada has none of her own, so there is nothing for her to give up.
    let refused = floor.open(&floor.scene.ada).await.problem();
    assert_eq!(refused.code, ErrorCode::ConflictExists);
    assert_eq!(refused.extra["limit"], "environment");
    assert_eq!(refused.extra["mine"], json!([]));
    assert_eq!(refused.extra["open"], 2);
    assert_eq!(refused.extra["people"], 2);
    // Whoever manages the environment closes any, and the new one opens.
    floor.close(&floor.scene.eve, theirs).await.ok();
    floor.open(&floor.scene.ada).await.ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_cannot_give_up_a_shell_that_is_somebody_elses() {
    let floor = Floor::new().await;
    let eves = floor.shell_of(&floor.scene.eve).await.session.id;
    let refused = floor
        .open_with(
            &floor.scene.dan,
            Some(floor.thread),
            json!({ "cols": 80, "rows": 24, "close": [eves] }),
        )
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(floor.row(eves).await.is_live(), "it was not closed");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_list_is_the_callers_own_shells_whichever_thread_each_came_from_and_a_manager_is_shown_everybodys()
 {
    let floor = Floor::new().await;
    // Bea may open shells, by the row of the environment's page, beside a thread she may look at, and
    // manages nothing.
    let bea = &floor.scene.bea;
    floor.names_on_the_environment(bea, "terminal.manage").await;
    floor.names_on_the_thread(bea, "thread.view").await;
    let from_a_thread = floor.shell_of(bea).await.session.id;
    let from_the_page = floor
        .open_with(&floor.scene.eve, None, sized())
        .await
        .ok()
        .session
        .id;
    let beas = floor.list(bea, None).await;
    assert_eq!(
        beas.iter().map(|each| each.id).collect::<Vec<_>>(),
        [from_a_thread]
    );
    let managers = floor.list_from_the_page(&floor.scene.eve).await;
    assert_eq!(
        managers.len(),
        2,
        "everybody's exist, for whoever manages the environment"
    );
    assert!(managers.iter().any(|each| each.id == from_the_page));
    let both: Vec<_> = managers
        .iter()
        .map(|each| each.opened_by.username.as_str())
        .collect();
    assert!(both.contains(&"bea") && both.contains(&"eve"));
    // Without a thread the list is the same: a thread says where the caller works and gives nothing, so
    // what the row of the environment's page lets Bea see is what she is shown either way.
    let by_the_page = floor.list_from_the_page(bea).await;
    assert_eq!(
        by_the_page.iter().map(|each| each.id).collect::<Vec<_>>(),
        [from_a_thread]
    );
    // A viewer holds the View of the shells and not the Manage that lists them: refused with what is
    // missing on the environment, with a thread named and without.
    let as_viewer = |thread: Option<orch_types::Id>| {
        let call = floor
            .scene
            .rig
            .world
            .app
            .call(
                orch_api_contract::terminals::EnvironmentTerminalList,
                &[&floor.environment],
            )
            .token(&floor.scene.cai.token);
        match thread {
            Some(thread) => call.raw_query(&format!("thread={thread}")),
            None => call,
        }
    };
    for thread in [Some(floor.thread), None] {
        let viewer = as_viewer(thread).send().await.problem();
        assert_eq!(
            viewer.code,
            ErrorCode::AuthzDenied,
            "a viewer is shown none, with {thread:?} named"
        );
        let missing = viewer.extra["missing"].to_string();
        assert!(missing.contains("terminal.manage"), "{missing}");
        assert!(missing.contains("environment"), "{missing}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_is_newest_first_and_pages_on() {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(floor.shell_of(dan).await.session.id);
        floor.scene.rig.world.advance(Duration::seconds(1));
    }
    let query = format!("thread={}&limit=2", floor.thread);
    let first = floor
        .scene
        .rig
        .world
        .app
        .call(
            orch_api_contract::terminals::EnvironmentTerminalList,
            &[&floor.environment],
        )
        .token(&dan.token)
        .raw_query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        first.items.iter().map(|each| each.id).collect::<Vec<_>>(),
        [ids[2], ids[1]]
    );
    let cursor = first.next_cursor.unwrap();
    let second = floor
        .scene
        .rig
        .world
        .app
        .call(
            orch_api_contract::terminals::EnvironmentTerminalList,
            &[&floor.environment],
        )
        .token(&dan.token)
        .raw_query(&format!("{query}&cursor={cursor}"))
        .send()
        .await
        .ok();
    assert_eq!(
        second.items.iter().map(|each| each.id).collect::<Vec<_>>(),
        [ids[0]]
    );
    assert!(second.next_cursor.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_opener_is_given_a_ticket_to_attach_to_a_shell_whatever_anybody_else_holds() {
    let floor = Floor::new().await;
    let shell = floor.shell_of(&floor.scene.dan).await.session.id;
    let own = floor.ticket(&floor.scene.dan, shell).await.ok();
    assert!(!own.ticket.is_empty());
    assert_eq!(
        own.ticket_expires_at.get() - floor.scene.rig.world.now(),
        Duration::seconds(30)
    );
    // A manager of the environment sees that it exists, and is given nothing.
    assert_eq!(
        floor.ticket(&floor.scene.eve, shell).await.problem().code,
        ErrorCode::NotFound
    );
    // Nor is its owner, who holds everything there is.
    assert_eq!(
        floor.ticket(&floor.scene.ada, shell).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        floor.ticket(&floor.scene.bea, shell).await.problem().code,
        ErrorCode::NotFound
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_opener_who_lost_what_opened_the_shell_is_given_no_ticket_and_the_shell_is_closed() {
    let floor = Floor::new().await;
    let shell = floor.shell_of(&floor.scene.dan).await.session.id;
    // Dan's role is taken away, which moves the epoch.
    let org = floor.scene.tree.org.base.id;
    let mut conn = floor.pool().acquire().await.unwrap();
    let role: orch_types::Id = sqlx::query_scalar(
        "SELECT id FROM node_roles WHERE node_id = $1 AND subject_id = $2 AND removed_at IS NULL",
    )
    .bind(org)
    .bind(floor.scene.dan.id())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    orch_db::queries::access::testing::take_role_away(
        &mut conn,
        role,
        floor.scene.ada.id(),
        floor.scene.rig.world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let refused = floor.ticket(&floor.scene.dan, shell).await.problem();
    assert!(
        matches!(refused.code, ErrorCode::AuthzDenied | ErrorCode::NotFound),
        "{refused:?}"
    );
    let row = floor.row(shell).await;
    assert_eq!(row.state, "closed");
    assert_eq!(row.close_reason.as_deref(), Some("revoked"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_opener_and_whoever_manages_the_environment_close_a_shell_and_nobody_else_does() {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    let first = floor.shell_of(dan).await.session.id;
    let second = floor.shell_of(dan).await.session.id;
    // A stranger is told nothing of it.
    assert_eq!(
        floor.close(&floor.scene.bea, first).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        floor.close(&floor.scene.cai, first).await.problem().code,
        ErrorCode::NotFound
    );
    assert!(floor.row(first).await.is_live());
    let by_opener = floor.close(dan, first).await.ok();
    assert_eq!(by_opener.state, TerminalState::Closed);
    assert_eq!(by_opener.close_reason.as_deref(), Some("closed_by_user"));
    assert!(by_opener.closed_at.is_some() && by_opener.closes_at.is_none());
    let by_manager = floor.close(&floor.scene.eve, second).await.ok();
    assert_eq!(by_manager.state, TerminalState::Closed);
    // Closing what is closed answers with it as it is, and the machine was asked once for each.
    let again = floor.close(dan, first).await.ok();
    assert_eq!(again.closed_at, by_opener.closed_at);
    assert_eq!(floor.fake.closed.lock().unwrap().len(), 2);
    let lines = floor.history().await;
    assert_eq!(
        lines
            .iter()
            .filter(|(action, _)| action == "terminal.closed")
            .count(),
        2,
        "one line for each end: {lines:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_line_of_a_shell_that_closed_says_who_for_how_long_how_it_ended_and_how_much_it_printed()
 {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    let shell = floor.shell_of(dan).await.session.id;
    floor
        .fake
        .shell(shell)
        .unwrap()
        .print_alone("a line of output\n");
    floor.scene.rig.world.advance(Duration::minutes(42));
    floor.close(dan, shell).await.ok();
    let lines = floor.history().await;
    let closed = &lines
        .iter()
        .find(|(action, _)| action == "terminal.closed")
        .unwrap()
        .1;
    assert!(
        closed.starts_with("dan's shell closed after 42 minutes"),
        "{closed}"
    );
    assert!(closed.contains("it was closed"), "{closed}");
    assert!(closed.contains("17 bytes printed"), "{closed}");
    assert!(closed.contains("nothing of it kept"), "{closed}");
    let details = floor.details("terminal.closed").await;
    assert_eq!(details[0]["seconds"], 42 * 60);
    assert_eq!(details[0]["how"], "closed_by_user");
    assert_eq!(details[0]["bytes_out"], 17);
    assert_eq!(details[0]["kept"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_shell_last_printed_is_read_by_its_opener_and_its_managers_and_a_read_by_another_is_a_line_once_a_day()
 {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    floor.store.knows_secret("hunter2");
    let shell = floor.shell_of(dan).await.session.id;
    *floor.fake.kept.lock().unwrap() = Some(floor.store.uploaded(b"build ok\npassword hunter2\n"));
    let closed = floor.close(dan, shell).await.ok();
    assert!(closed.recorded);
    let read = |who: &places::Who| {
        floor
            .scene
            .rig
            .world
            .app
            .call(TerminalRecordingGet, &[&shell])
            .token(&who.token)
            .send_bytes()
    };
    let own = read(dan).await;
    assert_eq!(own.status, 200);
    assert_eq!(own.headers["content-type"], "application/octet-stream");
    assert_eq!(
        String::from_utf8_lossy(&own.raw),
        "build ok\npassword [secret]\n",
        "the values of the secrets are taken out before it is read"
    );
    let reads = |action: &'static str| {
        let pool = floor.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_log WHERE action = $1")
                .bind(action)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        reads("terminal.recording_read").await,
        0,
        "the opener's own read is no line"
    );
    // A manager of the environment reads it too, and that is a line, once for that reader and day.
    assert_eq!(read(&floor.scene.eve).await.status, 200);
    assert_eq!(read(&floor.scene.eve).await.status, 200);
    assert_eq!(reads("terminal.recording_read").await, 1);
    floor.scene.rig.world.advance(Duration::days(1));
    assert_eq!(read(&floor.scene.eve).await.status, 200);
    assert_eq!(reads("terminal.recording_read").await, 2);
    let line = floor
        .history()
        .await
        .into_iter()
        .find(|(action, _)| action == "terminal.recording_read")
        .unwrap();
    assert_eq!(line.1, "eve read what dan's shell last printed");
    // Nobody else does.
    assert_eq!(read(&floor.scene.cai).await.status, 404);
    assert_eq!(read(&floor.scene.bea).await.status, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_read_of_a_shell_that_runs_or_that_kept_nothing() {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    let kept_nothing = floor.shell_of(dan).await.session.id;
    let running = floor.shell_of(dan).await.session.id;
    floor.close(dan, kept_nothing).await.ok();
    for shell in [kept_nothing, running] {
        let read = floor
            .scene
            .rig
            .world
            .app
            .call(TerminalRecordingGet, &[&shell])
            .token(&dan.token)
            .send_bytes()
            .await;
        assert_eq!(read.status, 404, "{shell}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn where_the_setting_asks_for_no_recording_none_is_asked_of_the_machine_or_kept() {
    let floor = Floor::new().await;
    floor
        .scene
        .rig
        .world
        .set_at(
            Target::Node(floor.scene.tree.topic.base.id),
            "terminal.record",
            json!(false),
        )
        .await;
    let made = floor.shell_of(&floor.scene.dan).await;
    assert!(!made.session.recorded);
    assert!(
        !floor.fake.opened.lock().unwrap()[0].record,
        "the machine was told not to keep it"
    );
    let closed = floor.close(&floor.scene.dan, made.session.id).await.ok();
    assert!(!closed.recorded);
    let details = floor.details("terminal.closed").await;
    assert_eq!(
        details[0]["kept"], false,
        "the line is there, with no output"
    );
}

/// Lets the agent of the scene's threads declare a terminal of its own.
async fn agent_declares(floor: &Floor, declared: &str) {
    sqlx::query(
        "UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,agent_terminal}', to_jsonb($2::text))
          WHERE id = $1",
    )
    .bind(floor.scene.on.agent)
    .bind(declared)
    .execute(floor.pool())
    .await
    .unwrap();
}

async fn agent_ticket(
    floor: &Floor,
    who: &places::Who,
    control: bool,
) -> orch_api_core::testing::Reply<orch_api_contract::terminals::TerminalTicket> {
    floor
        .scene
        .rig
        .world
        .app
        .call(AgentTerminalTicketCreate, &[&floor.thread])
        .token(&who.token)
        .body(&AgentTerminalTicketCreateBody {
            control: Some(control),
        })
        .send()
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agents_own_terminal_is_watched_with_terminal_view_and_typed_into_with_terminal_manage()
{
    let floor = Floor::new().await;
    agent_declares(&floor, "interactive").await;
    let watching = agent_ticket(&floor, &floor.scene.cai, false).await.ok();
    assert!(!watching.ticket.is_empty());
    let typing = agent_ticket(&floor, &floor.scene.cai, true).await.problem();
    assert_eq!(typing.code, ErrorCode::AuthzDenied);
    assert!(
        typing.extra["missing"]
            .to_string()
            .contains("terminal.manage"),
        "{typing:?}"
    );
    agent_ticket(&floor, &floor.scene.dan, true).await.ok();
    let stranger = agent_ticket(&floor, &floor.scene.bea, false)
        .await
        .problem();
    assert!(
        matches!(stranger.code, ErrorCode::AuthzDenied | ErrorCode::NotFound),
        "{stranger:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_declares_no_terminal_of_its_own_gives_no_ticket_for_it() {
    let floor = Floor::new().await;
    let none = agent_ticket(&floor, &floor.scene.dan, false)
        .await
        .problem();
    assert_eq!(none.code, ErrorCode::CapabilityUnsupported);
    agent_declares(&floor, "view").await;
    agent_ticket(&floor, &floor.scene.dan, false).await.ok();
    let more = agent_ticket(&floor, &floor.scene.dan, true).await.problem();
    assert_eq!(
        more.code,
        ErrorCode::CapabilityUnsupported,
        "a terminal that can only be watched cannot be typed into"
    );
}

#[test]
fn every_request_about_shells_is_bound() {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let bound: HashSet<&str> = registry.bound().iter().map(|route| route.name).collect();
    for route in orch_api_contract::terminals::ROUTES {
        assert!(bound.contains(route.name), "{} is not bound", route.name);
    }
    assert_eq!(orch_api_contract::terminals::ROUTES.len(), 8);
}
