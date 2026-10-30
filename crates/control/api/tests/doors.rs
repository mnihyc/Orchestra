//! The four doors of an environment over HTTP: each of the eight rows alone opens what it opens, a thread opens none of them, somebody named only on a thread is let in by the tick and no further, a manager holds all four with nobody named, the shipped rows read by role in both placements, and a restricted thread's files are read by whoever looks into its environment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use orch_api_contract::access::{PermissionPage, PermissionPageGet, PermissionPageUpdate};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    Environment, EnvironmentGet, EnvironmentReach, EnvironmentReachGet, EnvironmentSupport,
    EnvironmentSyncGet, EnvironmentSyncGetQuery, EnvironmentSyncJob, ReachPagePermission,
    ReachThrough, ThroughThreadQuery,
};
use orch_api_contract::processes::{
    EnvironmentProcessList, EnvironmentProcessListReply, EnvironmentProcessSignal,
    EnvironmentProcessSignalBody, ProcessSignal,
};
use orch_api_contract::terminals::{EnvironmentTerminalCreate, EnvironmentTerminalCreateReply};
use orch_api_contract::workspace::{
    DirListing, EnvironmentFileContentPut, EnvironmentFileContentPutQuery, EnvironmentFileList,
    EnvironmentFileListQuery, FileStat,
};
use orch_api_core::testing::Reply;
use orch_db::queries::jobs::{self as jobs, NewJob};
use orch_types::Id;
use places::Who;
use serde_json::json;
use workspace::Site;

const STARTED: &str = "2026-10-08T17:04:05.123Z";

async fn live_site() -> Site {
    let mut site = Site::new().await;
    site.disk.put("web/README.md", b"# Web\n");
    *site.disk.processes.lock().unwrap() = vec![json!({
        "pid": 40, "ppid": 1, "name": "postgres", "command": "postgres --serve", "user": "agent",
        "state": "S", "cpu_percent": 1.5, "memory_bytes": 1_048_576, "started_at": STARTED,
        "thread_id": null, "item_id": null
    })];
    site.power_on();
    site
}

// ---------------------------------------------------------------- the requests, without a thread

async fn list(site: &Site, by: &Who) -> Reply<DirListing> {
    site.app()
        .call(EnvironmentFileList, &[&site.environment])
        .token(&by.token)
        .query(&EnvironmentFileListQuery {
            thread: None,
            path: "web".to_owned(),
            source: None,
            mount: None,
            cursor: None,
            limit: None,
        })
        .send()
        .await
}

async fn write(site: &Site, by: &Who, path: &str) -> Reply<FileStat> {
    site.app()
        .call(EnvironmentFileContentPut, &[&site.environment])
        .token(&by.token)
        .query(&EnvironmentFileContentPutQuery {
            thread: None,
            path: path.to_owned(),
            expected_hash: Some("none".to_owned()),
            blob: None,
            upload_batch: None,
        })
        .bytes(b"written")
        .send()
        .await
}

async fn processes(site: &Site, by: &Who) -> Reply<EnvironmentProcessListReply> {
    site.app()
        .call(EnvironmentProcessList, &[&site.environment])
        .token(&by.token)
        .query(&ThroughThreadQuery { thread: None })
        .send()
        .await
}

async fn signal(site: &Site, by: &Who) -> Reply<()> {
    site.app()
        .call(EnvironmentProcessSignal, &[&site.environment, &40_u32])
        .token(&by.token)
        .query(&ThroughThreadQuery { thread: None })
        .body(&EnvironmentProcessSignalBody {
            signal: ProcessSignal::Term,
            started_at: STARTED.parse().unwrap(),
        })
        .send()
        .await
}

async fn shell(site: &Site, by: &Who) -> Reply<EnvironmentTerminalCreateReply> {
    site.app()
        .call(EnvironmentTerminalCreate, &[&site.environment])
        .token(&by.token)
        .query(&ThroughThreadQuery { thread: None })
        .raw_body(&json!({ "cols": 80, "rows": 24 }))
        .send()
        .await
}

/// Whether the request was refused because the caller lacks a permission: not for anything else, such
/// as a machine that takes no shell.
fn denied<T: serde::de::DeserializeOwned + std::fmt::Debug>(reply: Reply<T>) -> bool {
    if reply.status.is_success() {
        return false;
    }
    reply.problem().code == ErrorCode::AuthzDenied
}

async fn environment_as(site: &Site, by: &Who) -> Reply<Environment> {
    site.app()
        .call(EnvironmentGet, &[&site.environment])
        .token(&by.token)
        .send()
        .await
}

async fn reach_as(site: &Site, by: &Who) -> EnvironmentReach {
    site.app()
        .call(EnvironmentReachGet, &[&site.environment])
        .token(&by.token)
        .send()
        .await
        .ok()
}

// ---------------------------------------------------------------- naming people

/// Names `user` in the row `permission` of the page of `object`, as `by`.
async fn name_row(site: &Site, object: &str, id: Id, by: &Who, user: &Who, permission: &str) {
    site.app()
        .call(PermissionPageUpdate, &[&object, &id])
        .token(&by.token)
        .raw_body(&json!({ "rows": [{
            "permission": permission,
            "add": [{ "holder": { "kind": "user", "id": user.id() } }]
        }] }))
        .send()
        .await
        .ok();
}

/// Names `user` on the page of `object` with one of the three choices, as `by`.
async fn name_choice(site: &Site, object: &str, id: Id, by: &Who, user: &Who, choice: &str) {
    site.app()
        .call(PermissionPageUpdate, &[&object, &id])
        .token(&by.token)
        .raw_body(&json!({ "name": [{
            "holders": [{ "kind": "user", "id": user.id() }],
            "choice": choice
        }] }))
        .send()
        .await
        .ok();
}

async fn page_of(site: &Site, object: &str, id: Id, by: &Who) -> PermissionPage {
    site.app()
        .call(PermissionPageGet, &[&object, &id])
        .token(&by.token)
        .send()
        .await
        .ok()
}

// ---------------------------------------------------------------- the rows

/// What each of the eight rows lets a person do who is named in it and nowhere else: look at the files,
/// change them, see the processes, signal one, open a shell; and the browsers and the displays, which
/// no request of this part serves.
#[tokio::test]
async fn each_of_the_eight_rows_of_the_four_doors_alone_opens_what_it_opens_and_nothing_else() {
    let site = live_site().await;
    // (row, what a person named in it alone holds on the environment, in the order of the catalogue,
    // then: list, write, processes, signal, shell)
    let table = [
        (
            "files.view",
            vec!["files.view"],
            [true, false, false, false, false],
        ),
        (
            "files.manage",
            vec!["files.view", "files.manage"],
            [true, true, false, false, false],
        ),
        (
            "terminal.view",
            vec!["terminal.view"],
            [false, false, true, false, false],
        ),
        (
            "terminal.manage",
            vec!["terminal.view", "terminal.manage"],
            [false, false, true, true, true],
        ),
        ("browser.view", vec!["browser.view"], [false; 5]),
        (
            "browser.manage",
            vec!["browser.view", "browser.manage"],
            [false; 5],
        ),
        ("vnc.view", vec!["vnc.view"], [false; 5]),
        ("vnc.manage", vec!["vnc.view", "vnc.manage"], [false; 5]),
    ];
    for (index, (row, held, opens)) in table.into_iter().enumerate() {
        let guest = places::person(&site.world, &format!("guest_{index}")).await;
        name_row(
            &site,
            "environments",
            site.environment,
            &site.ada,
            &guest,
            row,
        )
        .await;
        let seen = environment_as(&site, &guest).await.ok();
        assert_eq!(
            seen.access.permissions, held,
            "{row}: it and the level it holds, not the environment, not another door"
        );
        assert_eq!(
            !denied(list(&site, &guest).await),
            opens[0],
            "{row}: look at the files"
        );
        assert_eq!(
            !denied(write(&site, &guest, &format!("web/by-{index}.txt")).await),
            opens[1],
            "{row}: change the files"
        );
        assert_eq!(
            !denied(processes(&site, &guest).await),
            opens[2],
            "{row}: see the processes"
        );
        assert_eq!(
            !denied(signal(&site, &guest).await),
            opens[3],
            "{row}: signal one"
        );
        assert_eq!(
            !denied(shell(&site, &guest).await),
            opens[4],
            "{row}: open a shell"
        );
        // Everybody who reaches in is told by which row.
        let reach = reach_as(&site, &site.ada).await;
        let person = reach
            .people
            .iter()
            .find(|person| person.user.id == guest.id())
            .unwrap_or_else(|| panic!("{row}: listed among those who reach in"));
        assert_eq!(person.through.len(), 1, "{row}: by one way");
        assert!(
            matches!(
                &person.through[0],
                ReachThrough::Page { permission } if serde_json::to_value(permission).unwrap() == json!(row)
            ),
            "{row}: {:?}",
            person.through
        );
    }
}

#[tokio::test]
async fn a_thread_gives_none_of_the_doors_whatever_is_held_on_it() {
    let site = live_site().await;
    // Everything on the thread, and nothing on the environment.
    let guest = places::person(&site.world, "guest").await;
    name_choice(
        &site,
        "nodes",
        site.thread,
        &site.ada,
        &guest,
        "full_manage",
    )
    .await;
    // The conversation is theirs. The files, the processes and a shell are not, and the refusal names
    // what is missing on the environment.
    let by_thread = site
        .app()
        .call(EnvironmentFileList, &[&site.environment])
        .token(&guest.token)
        .query(&EnvironmentFileListQuery {
            thread: Some(site.thread),
            path: "web".to_owned(),
            source: None,
            mount: None,
            cursor: None,
            limit: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(by_thread.code, ErrorCode::AuthzDenied);
    let missing = places::missing_permissions(&by_thread);
    assert!(missing.contains(&"files.view".to_owned()), "{missing:?}");
    assert!(
        by_thread.extra["missing"]
            .to_string()
            .contains("environment"),
        "{by_thread:?}"
    );
    assert!(denied(list(&site, &guest).await));
    assert!(denied(processes(&site, &guest).await));
    assert!(denied(shell(&site, &guest).await));
    assert!(denied(write(&site, &guest, "web/x.txt").await));
    assert_eq!(
        site.sent(orch_link_contract::commands::CommandKind::FsWrite),
        0
    );
    // The environment is theirs to see as far as the thread goes and no further: they hold nothing on
    // it, and who reaches into it is not theirs to ask.
    let seen = environment_as(&site, &guest).await.ok();
    assert!(seen.access.permissions.is_empty(), "{:?}", seen.access);
    assert_eq!(reach_status(&site, &guest).await, 404);
}

#[tokio::test]
async fn somebody_named_only_on_a_thread_is_let_in_by_the_tick_and_no_further() {
    let site = live_site().await;
    let guest = places::person(&site.world, "guest").await;
    name_choice(
        &site,
        "nodes",
        site.thread,
        &site.ada,
        &guest,
        "full_manage",
    )
    .await;
    // The page of the thread shows the environment beside the names, and that nobody is let in.
    let page = page_of(&site, "nodes", site.thread, &site.ada).await;
    let beside: Vec<_> = page
        .depends
        .iter()
        .filter(|dependency| dependency.object.kind == "environment")
        .map(|dependency| {
            (
                dependency.for_permission.as_str(),
                dependency.permission.as_str(),
                dependency.choice,
            )
        })
        .collect();
    // Looking in is the environment's Can view, given as that choice; setting the thread up goes
    // with the Manage of its files and of its shells.
    assert_eq!(
        beside,
        [
            (
                "thread.view",
                "environment.view",
                Some(orch_api_contract::access::AccessChoice::FullView)
            ),
            ("thread.manage", "files.manage", None),
            ("thread.manage", "terminal.manage", None),
        ]
    );
    assert!(page.depends.iter().all(|dependency| dependency.can_give));
    // Ticking "Let them into" is those changes of the environment's page, by Ada, who may give them.
    name_choice(
        &site,
        "environments",
        site.environment,
        &site.ada,
        &guest,
        "full_view",
    )
    .await;
    name_row(
        &site,
        "environments",
        site.environment,
        &site.ada,
        &guest,
        "files.manage",
    )
    .await;
    name_row(
        &site,
        "environments",
        site.environment,
        &site.ada,
        &guest,
        "terminal.manage",
    )
    .await;
    assert!(list(&site, &guest).await.status.is_success());
    assert!(
        write(&site, &guest, "web/by-guest.txt")
            .await
            .status
            .is_success()
    );
    assert!(processes(&site, &guest).await.status.is_success());
    assert!(!denied(shell(&site, &guest).await));
    // No further: not the environment, and not the Manage of browsers or displays.
    let seen = environment_as(&site, &guest).await.ok();
    for held in ["environment.manage", "browser.manage", "vnc.manage"] {
        assert!(
            !seen.access.permissions.contains(&held.to_owned()),
            "{held}: {:?}",
            seen.access.permissions
        );
    }
    assert!(seen.access.permissions.contains(&"browser.view".to_owned()));
}

#[tokio::test]
async fn whoever_manages_the_environment_holds_all_four_doors_with_nobody_named_in_them() {
    let site = live_site().await;
    // Dev is a Developer of the org: as shipped the Developers manage an environment made from a
    // template, and nobody is named in any door.
    let page = page_of(&site, "environments", site.environment, &site.ada).await;
    for row in &page.rows {
        if row.permission.starts_with("files.")
            || row.permission.starts_with("terminal.")
            || row.permission.starts_with("browser.")
            || row.permission.starts_with("vnc.")
        {
            assert!(
                row.entries.is_empty(),
                "nobody is named in {}",
                row.permission
            );
        }
    }
    let seen = environment_as(&site, &site.dev).await.ok();
    for held in [
        "environment.manage",
        "files.manage",
        "terminal.manage",
        "browser.manage",
        "vnc.manage",
    ] {
        assert!(
            seen.access.permissions.contains(&held.to_owned()),
            "{held}: {:?}",
            seen.access.permissions
        );
    }
    let written = write(&site, &site.dev, "web/by-dev.txt").await;
    assert!(written.status.is_success(), "{:?}", written.body);
    assert!(signal(&site, &site.dev).await.status.is_success());
    assert!(!denied(shell(&site, &site.dev).await));
    // The page says it so: by managing, and not once for each door.
    let reach = reach_as(&site, &site.ada).await;
    let dev = reach
        .people
        .iter()
        .find(|person| person.user.id == site.dev.id())
        .unwrap();
    assert!(dev.through.iter().any(|way| matches!(
        way,
        ReachThrough::Page {
            permission: ReachPagePermission::EnvironmentManage
        }
    )));
    assert!(
        dev.through
            .iter()
            .filter(|way| matches!(way, ReachThrough::Page { .. }))
            .count()
            == 1,
        "{:?}",
        dev.through
    );
    // A viewer is among those who look in, by the four rows that ship to Viewers, and manages nothing.
    let vic = reach
        .people
        .iter()
        .find(|person| person.user.id == site.vic.id())
        .unwrap();
    let doors: Vec<String> = vic
        .through
        .iter()
        .filter_map(|way| match way {
            ReachThrough::Page { permission } => Some(
                serde_json::to_value(permission)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(
        doors,
        ["browser.view", "files.view", "terminal.view", "vnc.view"],
        "the four rows that ship to Viewers, in the order of their names"
    );
}

#[tokio::test]
async fn the_shipped_rows_of_the_doors_are_read_by_role_in_both_placements() {
    let site = live_site().await;
    let defaults = site
        .app()
        .call(orch_api_contract::access::NodeDefaultsGet, &[&site.topic])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    let holders = |permission: &str, profile: &str| -> Vec<String> {
        let row = defaults
            .rows
            .iter()
            .find(|row| {
                row.permission == permission
                    && serde_json::to_value(row.profile).unwrap() == json!(profile)
            })
            .unwrap_or_else(|| panic!("a row of {permission} for {profile}"));
        row.inherited
            .as_ref()
            .map(|inherited| {
                inherited
                    .holders
                    .iter()
                    .map(|holder| serde_json::to_value(holder).unwrap()["role"].to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    for profile in ["managed", "connected"] {
        for view in ["files.view", "terminal.view", "browser.view", "vnc.view"] {
            assert_eq!(
                holders(view, profile),
                ["\"viewer\""],
                "{view} for {profile}"
            );
        }
        for nobody in ["browser.manage", "vnc.manage"] {
            assert!(
                holders(nobody, profile).is_empty(),
                "{nobody} for {profile}: whoever manages the environment drives it, nobody else"
            );
        }
    }
    for door in ["files.manage", "terminal.manage"] {
        assert_eq!(holders(door, "managed"), ["\"developer\""], "{door}");
        assert!(
            holders(door, "connected").is_empty(),
            "{door}: on a person's machine nobody, until its page says so"
        );
    }
}

#[tokio::test]
async fn a_restricted_thread_keeps_its_conversation_and_not_its_files_and_its_page_says_how_many_look_in()
 {
    let site = live_site().await;
    // The thread is made private to Ada: Vic reads none of it.
    site.app()
        .call(PermissionPageUpdate, &[&"nodes", &site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({ "whole": "private" }))
        .send()
        .await
        .ok();
    let closed = site
        .app()
        .call(orch_api_contract::nodes::NodeGet, &[&site.thread])
        .token(&site.vic.token)
        .send()
        .await;
    assert!(
        !closed.status.is_success(),
        "the conversation is its people's"
    );
    // The files of the environment it runs in are read by whoever looks into it.
    assert!(list(&site, &site.vic).await.status.is_success());
    // The page says how many that is, and that other threads work there.
    let page = page_of(&site, "nodes", site.thread, &site.ada).await;
    let reaches = page
        .reaches
        .expect("a thread in an environment says what it reaches");
    assert_eq!(reaches.environment.id, site.environment);
    let readers = reach_as(&site, &site.ada)
        .await
        .people
        .into_iter()
        .filter(|person| {
            person.through.iter().any(|way| {
                matches!(
                    way,
                    ReachThrough::Owner
                        | ReachThrough::Page {
                            permission: ReachPagePermission::FilesView
                                | ReachPagePermission::FilesManage
                                | ReachPagePermission::EnvironmentManage,
                        }
                )
            })
        })
        .count();
    assert_eq!(
        usize::try_from(reaches.people).unwrap(),
        readers,
        "the people who read its files are the people the page counts"
    );
    assert!(reaches.people >= 3, "Ada, Vic and Dev at least");
    assert_eq!(reaches.threads, 0, "no other thread works there");
}

#[tokio::test]
async fn an_environment_is_read_through_a_thread_as_far_as_its_name_owner_place_state_and_job_and_no_further()
 {
    let site = live_site().await;
    let guest = places::person(&site.world, "guest").await;
    // Nothing on it and no thread that runs there to look at: refused, with what is lacking on it.
    let refused = environment_as(&site, &guest).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(
        places::missing_permissions(&refused).contains(&"environment.view".to_owned()),
        "{refused:?}"
    );
    assert_eq!(
        reach_status(&site, &guest).await,
        404,
        "who reaches in is for those who hold something there"
    );
    // A job under way on it, and a thread of the same topic that the guest is not named on.
    let job = {
        let mut conn = site.world.conn().await;
        jobs::insert(
            &mut conn,
            &NewJob {
                id: orch_types::new_id(),
                kind: jobs::kind::ENVIRONMENT_RECREATE,
                node_id: None,
                environment_id: Some(site.environment),
                provider_id: None,
                plan: &json!({}),
                progress: &json!({}),
                created_by: Some(site.ada.id()),
            },
            site.world.now(),
        )
        .await
        .unwrap()
    };
    let _unseen = site.another_thread(&site.ada, "elsewhere").await;
    // And something its machine reported of it, which only those who hold something on it are told.
    sqlx::query("UPDATE environments SET facts = '{\"os\": \"linux\"}' WHERE id = $1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    // Named on the thread to read it, and on nothing of the environment. The thread's topic is not theirs
    // to see yet, and the threads of a topic they may not view are counted and not shown.
    name_choice(&site, "nodes", site.thread, &site.ada, &guest, "full_view").await;
    let at_first = environment_as(&site, &guest).await.ok();
    assert!(at_first.threads.is_empty(), "{:?}", at_first.threads);
    assert_eq!(at_first.threads_hidden, 2);
    assert!(
        at_first.home.redacted,
        "its place is named to those who may know it"
    );
    name_choice(&site, "nodes", site.topic, &site.ada, &guest, "full_view").await;
    let whole = environment_as(&site, &site.ada).await.ok();
    let seen = environment_as(&site, &guest).await.ok();
    assert_eq!(
        (&seen.base, seen.org_id, &seen.home),
        (&whole.base, whole.org_id, &whole.home),
        "its name, owner, place and state are what everybody who reads it is told"
    );
    assert_eq!(seen.job.as_ref().map(|job| job.id), Some(job.id));
    assert_eq!(
        seen.threads
            .iter()
            .map(|each| each.thread.id)
            .collect::<Vec<_>>(),
        [site.thread],
        "the threads they may view, of the topics they may view"
    );
    assert_eq!(seen.threads_hidden, 1, "and how many more there are");
    assert!(
        seen.access.permissions.is_empty() && !seen.access.administers,
        "they hold nothing on it: {:?}",
        seen.access
    );
    // Nothing of what is behind it, which is there for whoever holds something on it.
    assert!(!whole.workspace_root.is_empty() && whole.facts != json!({}));
    assert_eq!(
        (&seen.workspace_root, &seen.config, &seen.facts),
        (&String::new(), &json!({}), &json!({}))
    );
    assert!(seen.machine.is_none() && seen.host.is_none() && seen.template.is_none());
    assert!(
        seen.mounts.is_empty()
            && seen.mcp_found.is_empty()
            && seen.granted_capabilities.is_empty()
            && seen.exec.offers.is_empty()
    );
    assert!(!seen.remote_write && !seen.terminal && !seen.browser && !seen.vnc);
    assert_eq!(seen.support, EnvironmentSupport::nothing_found());
    assert!(seen.last_activity_at.is_none() && seen.idle_stop_at.is_none());
    // The state behind its feed is read by the same door and holds the same cut of it, with the job
    // under way that the cut names.
    let state = site
        .app()
        .call(EnvironmentSyncGet, &[&site.environment])
        .token(&guest.token)
        .query(&EnvironmentSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(state.environment, seen);
    assert_eq!(
        state
            .jobs
            .iter()
            .map(|each| match each {
                EnvironmentSyncJob::Reference(job) => job.id,
                EnvironmentSyncJob::Full(_) =>
                    panic!("a thread-only viewer must not receive full job details"),
            })
            .collect::<Vec<_>>(),
        [job.id]
    );
    // The thread is no door: its files are refused, and the request for who reaches in is not theirs.
    assert!(denied(list(&site, &guest).await));
    assert_eq!(reach_status(&site, &guest).await, 404);
    // Somebody who holds something on it reads all of it, a viewer among them by what ships.
    let by_a_viewer = environment_as(&site, &site.vic).await.ok();
    assert_eq!(by_a_viewer.workspace_root, whole.workspace_root);
    assert!(
        by_a_viewer
            .access
            .permissions
            .contains(&"environment.view".to_owned())
    );
}

async fn reach_status(site: &Site, by: &Who) -> u16 {
    site.app()
        .call(EnvironmentReachGet, &[&site.environment])
        .token(&by.token)
        .send_bytes()
        .await
        .status
        .as_u16()
}
