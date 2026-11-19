//! What can be found out about a workspace without opening its files one by one over HTTP: the latest change to each path that agents made, a search by name that works while the machine is away and one inside the text that does not, what an archive holds and one of its entries for a viewer within the limits of a preview, and the state and differences of a repository.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use http::StatusCode;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::workspace::{
    ARCHIVE_LIST_LIMIT_MAX, ArchiveEntryKind, ArchiveFormat, EnvironmentArchiveEntryGet,
    EnvironmentArchiveEntryGetQuery, EnvironmentArchiveList, EnvironmentArchiveListQuery,
    EnvironmentChangeList, EnvironmentChangeListQuery, EnvironmentFileSearch,
    EnvironmentFileSearchQuery, EnvironmentFileSearchReply, EnvironmentGitDiffGet,
    EnvironmentGitDiffGetQuery, EnvironmentGitStatusGet, EnvironmentGitStatusGetQuery,
    FILE_SEARCH_LIMIT_MAX, FileSearchIn, GIT_DIFF_MAX_BYTES, GitUnavailable, TRUNCATED_HEADER,
};
use orch_config::ServerConfig;
use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode as MachineCode};
use orch_types::Id;
use places::Who;
use serde_json::json;
use workspace::Site;

async fn live_site() -> Site {
    let mut site = Site::new().await;
    site.disk.put(
        "web/src/main.rs",
        b"fn main() {\n    println!(\"hello\");\n}\n",
    );
    site.disk.put(
        "web/src/lib.rs",
        b"pub fn greet() -> &'static str {\n    \"Hello\"\n}\n",
    );
    site.disk
        .put("web/README.md", b"# Web\nSay hello to everybody.\n");
    site.disk.put("web/docs/guide.txt", b"A guide.\n");
    site.power_on();
    site
}

fn change(path: &str, kind: &str) -> serde_json::Value {
    json!({
        "path": path, "change": kind, "old_path": null, "diff": "@@ -1 +1 @@\n-a\n+b\n",
        "added": 1, "removed": 1, "binary": false, "truncated": false
    })
}

async fn changes(
    site: &Site,
    by: &Who,
    thread: Option<Id>,
    all: Option<bool>,
) -> orch_api_core::testing::Reply<orch_api_contract::workspace::EnvironmentChangeListReply> {
    site.app()
        .call(EnvironmentChangeList, &[&site.environment])
        .token(&by.token)
        .query(&EnvironmentChangeListQuery {
            thread,
            turn_id: None,
            all,
        })
        .send()
        .await
}

// ---------------------------------------------------------------- what agents changed

#[tokio::test]
async fn the_latest_change_to_each_path_is_listed_from_the_file_change_calls_of_the_thread() {
    let site = live_site().await;
    let first = site
        .file_change_item(
            site.thread,
            &[change("web/a.txt", "modify"), change("web/b.txt", "add")],
        )
        .await;
    site.world.advance(chrono::Duration::minutes(1));
    let second = site
        .file_change_item(site.thread, &[change("web/a.txt", "delete")])
        .await;

    let listed = changes(&site, &site.ada, Some(site.thread), None)
        .await
        .ok();
    let mut found: Vec<_> = listed
        .changes
        .iter()
        .map(|each| (each.change.path.clone(), each.change.change, each.item_id))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    use orch_api_contract::threads::FileChangeKind;
    assert_eq!(
        found,
        [
            ("web/a.txt".to_owned(), FileChangeKind::Delete, second),
            ("web/b.txt".to_owned(), FileChangeKind::Add, first),
        ],
        "one change for each path, the last one"
    );
    assert!(
        listed
            .changes
            .iter()
            .all(|each| each.thread_id == site.thread)
    );
    assert_eq!(listed.changes[0].change.added, 1);
}

#[tokio::test]
async fn the_changes_of_one_thread_are_not_those_of_another_unless_all_are_asked_for() {
    let site = live_site().await;
    let other = site.another_thread(&site.ada, "docs").await;
    site.file_change_item(site.thread, &[change("web/mine.txt", "add")])
        .await;
    site.file_change_item(other, &[change("docs/theirs.txt", "add")])
        .await;

    let mine = changes(&site, &site.ada, Some(site.thread), None)
        .await
        .ok();
    assert_eq!(mine.changes.len(), 1);
    assert_eq!(mine.changes[0].change.path, "web/mine.txt");

    let every = changes(&site, &site.ada, Some(site.thread), Some(true))
        .await
        .ok();
    let mut paths: Vec<_> = every
        .changes
        .iter()
        .map(|each| each.change.path.as_str())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, ["docs/theirs.txt", "web/mine.txt"]);

    // Without a thread, whoever manages the environment sees what every thread there changed.
    let all_of_them = changes(&site, &site.ada, None, None).await.ok();
    assert_eq!(all_of_them.changes.len(), 2);
}

#[tokio::test]
async fn the_changes_are_for_whoever_may_read_the_files() {
    let site = live_site().await;
    site.file_change_item(site.thread, &[change("web/a.txt", "add")])
        .await;
    assert_eq!(
        changes(&site, &site.vic, Some(site.thread), None)
            .await
            .ok()
            .changes
            .len(),
        1
    );
    let refused = changes(&site, &site.eve, Some(site.thread), None).await;
    assert!(!refused.status.is_success());
    // The changes of the files are the environment's, read by the row of its page that ships to Viewers,
    // with a thread named or none: the thread says where the caller works and gives nothing.
    assert_eq!(
        changes(&site, &site.vic, None, None)
            .await
            .ok()
            .changes
            .len(),
        1,
        "a viewer reads the changes by files.view of the environment"
    );
    let no_door = changes(&site, &site.eve, None, None).await.problem();
    assert_eq!(no_door.code, ErrorCode::AuthzDenied);
    assert!(
        places::missing_permissions(&no_door).contains(&"files.view".to_owned()),
        "{no_door:?}"
    );
}

// ---------------------------------------------------------------- searching

async fn search(
    site: &Site,
    by: &Who,
    query: EnvironmentFileSearchQuery,
) -> orch_api_core::testing::Reply<EnvironmentFileSearchReply> {
    let mut query = query;
    query.thread = Some(site.thread);
    site.app()
        .call(EnvironmentFileSearch, &[&site.environment])
        .token(&by.token)
        .query(&query)
        .send()
        .await
}

fn by_name(q: &str) -> EnvironmentFileSearchQuery {
    EnvironmentFileSearchQuery {
        thread: None,
        q: q.to_owned(),
        scope: None,
        path: None,
        glob: None,
        regex: None,
        case: None,
        limit: None,
    }
}

fn in_text(q: &str) -> EnvironmentFileSearchQuery {
    EnvironmentFileSearchQuery {
        scope: Some(FileSearchIn::Content),
        ..by_name(q)
    }
}

#[tokio::test]
async fn files_are_found_by_name_in_the_last_list_whether_or_not_the_machine_is_there() {
    let site = live_site().await;
    site.publish_manifest().await;
    for _ in 0..2 {
        let found = match search(&site, &site.vic, by_name("MAIN")).await.ok() {
            EnvironmentFileSearchReply::Names(entries) => entries,
            other @ EnvironmentFileSearchReply::Content(_) => panic!("{other:?}"),
        };
        assert_eq!(
            found
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["web/src/main.rs"],
            "a substring of the path, without regard to case"
        );
        site.power_off().await;
    }
    assert_eq!(
        site.sent(CommandKind::FsSearch),
        0,
        "the machine was not asked"
    );
}

#[tokio::test]
async fn a_search_is_for_at_least_two_characters() {
    let site = live_site().await;
    site.publish_manifest().await;
    assert_eq!(
        search(&site, &site.ada, by_name("m")).await.problem().code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        search(&site, &site.ada, by_name("  ")).await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn names_cannot_be_searched_where_no_list_is_kept_or_none_has_come_yet() {
    let site = live_site().await;
    // The machine is there and has not sent a list.
    assert_eq!(
        search(&site, &site.ada, by_name("main"))
            .await
            .problem()
            .code,
        ErrorCode::EnvironmentUnavailable
    );
    site.power_off().await;
    assert_eq!(
        search(&site, &site.ada, by_name("main"))
            .await
            .problem()
            .code,
        ErrorCode::SourceOffline
    );
    // A tree too large to index has no list at all.
    sqlx::query("UPDATE environments SET facts = '{\"index\": \"live\"}' WHERE id = $1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        search(&site, &site.ada, by_name("main"))
            .await
            .problem()
            .code,
        ErrorCode::WorkspaceNotIndexed
    );
}

#[tokio::test]
async fn the_lines_of_text_files_are_searched_in_the_environment_as_it_is_now() {
    let site = live_site().await;
    let found = match search(&site, &site.ada, in_text("hello")).await.ok() {
        EnvironmentFileSearchReply::Content(found) => found,
        other @ EnvironmentFileSearchReply::Names(_) => panic!("{other:?}"),
    };
    assert!(!found.truncated);
    let mut lines: Vec<_> = found
        .matches
        .iter()
        .map(|each| (each.path.as_str(), each.line))
        .collect();
    lines.sort_unstable();
    assert_eq!(
        lines,
        [
            ("web/README.md", 2),
            ("web/src/lib.rs", 2),
            ("web/src/main.rs", 2)
        ],
        "without regard to case, in the thread's own directory"
    );
    let body = &site.played.as_ref().unwrap().seen_of(CommandKind::FsSearch)[0].body;
    assert_eq!(
        body["path"], "web",
        "the thread's directory unless a directory is given"
    );
    assert_eq!(body["limit"], 200);
    assert_eq!(body["case"], false);
    assert_eq!(body["regex"], false);
}

#[tokio::test]
async fn a_search_in_the_text_keeps_to_the_directory_asked_for_and_carries_the_options_to_the_machine()
 {
    let site = live_site().await;
    let found = match search(
        &site,
        &site.ada,
        EnvironmentFileSearchQuery {
            path: Some("web/src".to_owned()),
            glob: Some("*.rs".to_owned()),
            case: Some(true),
            regex: Some(false),
            limit: Some(1),
            ..in_text("Hello")
        },
    )
    .await
    .ok()
    {
        EnvironmentFileSearchReply::Content(found) => found,
        other @ EnvironmentFileSearchReply::Names(_) => panic!("{other:?}"),
    };
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].path, "web/src/lib.rs");
    let body = &site.played.as_ref().unwrap().seen_of(CommandKind::FsSearch)[0].body;
    assert_eq!(body["path"], "web/src");
    assert_eq!(body["glob"], "*.rs");
    assert_eq!(body["case"], true);
    assert_eq!(body["limit"], 1);
}

#[tokio::test]
async fn how_many_matches_may_be_asked_for_is_bounded() {
    let site = live_site().await;
    search(
        &site,
        &site.ada,
        EnvironmentFileSearchQuery {
            limit: Some(100_000),
            ..in_text("hello")
        },
    )
    .await
    .ok();
    let body = &site.played.as_ref().unwrap().seen_of(CommandKind::FsSearch)[0].body;
    assert_eq!(body["limit"], FILE_SEARCH_LIMIT_MAX);
    let none = search(
        &site,
        &site.ada,
        EnvironmentFileSearchQuery {
            limit: Some(0),
            ..in_text("hello")
        },
    )
    .await
    .problem();
    assert_eq!(none.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_search_in_the_text_needs_the_machine_and_a_machine_that_cannot_do_it_says_so_plainly() {
    let site = live_site().await;
    site.disk.refuse_next(
        CommandKind::FsSearch,
        CommandError::new(MachineCode::Unsupported, "that is not a regular expression"),
    );
    let refused = search(
        &site,
        &site.ada,
        EnvironmentFileSearchQuery {
            regex: Some(true),
            ..in_text("(")
        },
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    site.power_off().await;
    assert_eq!(
        search(&site, &site.ada, in_text("hello"))
            .await
            .problem()
            .code,
        ErrorCode::SourceOffline
    );
}

#[tokio::test]
async fn searching_is_for_whoever_may_read_the_files() {
    let site = live_site().await;
    assert!(
        search(&site, &site.vic, in_text("hello"))
            .await
            .status
            .is_success()
    );
    let stranger = search(&site, &site.eve, in_text("hello")).await;
    assert!(!stranger.status.is_success());
    assert_eq!(
        site.sent(CommandKind::FsSearch),
        1,
        "only the viewer's search reached the machine"
    );
}

// ---------------------------------------------------------------- archives

async fn archive_list(
    site: &Site,
    by: &Who,
    path: &str,
    limit: Option<u32>,
) -> orch_api_core::testing::Reply<orch_api_contract::workspace::ArchiveListing> {
    site.app()
        .call(EnvironmentArchiveList, &[&site.environment])
        .token(&by.token)
        .query(&EnvironmentArchiveListQuery {
            thread: Some(site.thread),
            path: path.to_owned(),
            inside: Some("/docs/".to_owned()),
            limit,
        })
        .send()
        .await
}

async fn archive_entry(
    site: &Site,
    by: &Who,
    path: &str,
    entry: &str,
) -> orch_api_core::testing::RawReply {
    site.app()
        .call(EnvironmentArchiveEntryGet, &[&site.environment])
        .token(&by.token)
        .query(&EnvironmentArchiveEntryGetQuery {
            thread: Some(site.thread),
            path: path.to_owned(),
            entry: entry.to_owned(),
        })
        .send_bytes()
        .await
}

#[tokio::test]
async fn what_an_archive_holds_is_read_where_it_lies() {
    let site = live_site().await;
    site.disk.put("web/pack.zip", b"PK pretend archive");
    let listing = archive_list(&site, &site.vic, "web/pack.zip", None)
        .await
        .ok();
    assert_eq!(listing.format, ArchiveFormat::Zip);
    assert!(!listing.encrypted && !listing.truncated);
    assert_eq!(listing.entries.len(), 2);
    assert_eq!(listing.entries[0].kind, ArchiveEntryKind::Dir);
    let body = &site
        .played
        .as_ref()
        .unwrap()
        .seen_of(CommandKind::FsArchiveList)[0]
        .body;
    assert_eq!(body["path"], "web/pack.zip");
    assert_eq!(
        body["inside"], "docs",
        "a directory inside it, without the slashes around"
    );
    assert_eq!(body["limit"], 1_000);
    assert_eq!(
        site.sent(CommandKind::FsWrite),
        0,
        "nothing is unpacked into the workspace"
    );
}

#[tokio::test]
async fn how_many_entries_of_an_archive_are_listed_is_bounded_and_a_missing_archive_is_not_found() {
    let site = live_site().await;
    site.disk.put("web/pack.zip", b"PK");
    archive_list(&site, &site.ada, "web/pack.zip", Some(1_000_000))
        .await
        .ok();
    let body = &site
        .played
        .as_ref()
        .unwrap()
        .seen_of(CommandKind::FsArchiveList)[0]
        .body;
    assert_eq!(body["limit"], ARCHIVE_LIST_LIMIT_MAX);
    assert_eq!(
        archive_list(&site, &site.ada, "web/pack.zip", Some(0))
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        archive_list(&site, &site.ada, "web/none.zip", None)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        archive_list(&site, &site.ada, "../pack.zip", None)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn an_archive_is_read_by_whoever_may_read_the_files_and_only_while_the_machine_is_there() {
    let site = live_site().await;
    site.disk.put("web/pack.zip", b"PK");
    let stranger = archive_list(&site, &site.eve, "web/pack.zip", None).await;
    assert!(!stranger.status.is_success());
    site.power_off().await;
    assert_eq!(
        archive_list(&site, &site.ada, "web/pack.zip", None)
            .await
            .problem()
            .code,
        ErrorCode::SourceOffline
    );
    assert_eq!(
        archive_entry(&site, &site.ada, "web/pack.zip", "readme.txt")
            .await
            .problem()
            .code,
        ErrorCode::SourceOffline
    );
}

#[tokio::test]
async fn one_entry_of_an_archive_is_served_to_a_viewer_and_written_nowhere() {
    let site = live_site().await;
    site.disk.put("web/pack.zip", b"PK");
    let entry = archive_entry(&site, &site.vic, "web/pack.zip", "docs/readme.txt").await;
    assert_eq!(entry.status, StatusCode::OK);
    assert_eq!(entry.raw.as_ref(), b"inside docs/readme.txt");
    assert!(
        entry.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain"),
        "{:?}",
        entry.headers["content-type"]
    );
    assert_eq!(site.sent(CommandKind::FsWrite), 0);
    assert!(!site.disk.exists("docs/readme.txt"));
}

#[tokio::test]
async fn an_entry_name_must_name_a_file_inside_the_archive() {
    let site = live_site().await;
    site.disk.put("web/pack.zip", b"PK");
    for entry in ["", "/", "../etc/passwd", "a//b", "docs/../../x"] {
        let refused = archive_entry(&site, &site.ada, "web/pack.zip", entry)
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{entry:?}");
    }
    assert_eq!(site.sent(CommandKind::FsArchiveRead), 0);
}

#[tokio::test]
async fn an_entry_larger_than_a_preview_shows_is_refused_with_the_limit() {
    let mut config = ServerConfig::default();
    config.limits.preview_bytes = 10;
    let mut site = Site::with_config(config).await;
    site.disk.put("web/pack.zip", b"PK");
    site.power_on();
    let small = archive_entry(&site, &site.ada, "web/pack.zip", "a").await;
    assert_eq!(small.status, StatusCode::OK, "{:?}", small.raw);
    let large = archive_entry(&site, &site.ada, "web/pack.zip", "a/long/enough/name.txt")
        .await
        .problem();
    assert_eq!(large.code, ErrorCode::RequestTooLarge);
    assert_eq!(large.extra["limit"], 10);
}

// ---------------------------------------------------------------- repositories

async fn git_status(
    site: &Site,
    by: &Who,
    path: Option<&str>,
) -> orch_api_core::testing::Reply<orch_api_contract::workspace::GitStatus> {
    site.app()
        .call(EnvironmentGitStatusGet, &[&site.environment])
        .token(&by.token)
        .query(&EnvironmentGitStatusGetQuery {
            thread: Some(site.thread),
            path: path.map(str::to_owned),
        })
        .send()
        .await
}

async fn git_diff(site: &Site, by: &Who, staged: Option<bool>) -> orch_api_core::testing::RawReply {
    site.app()
        .call(EnvironmentGitDiffGet, &[&site.environment])
        .token(&by.token)
        .query(&EnvironmentGitDiffGetQuery {
            thread: Some(site.thread),
            path: None,
            staged,
        })
        .send_bytes()
        .await
}

#[tokio::test]
async fn the_state_of_the_repository_at_the_root_is_read_by_the_environments_own_git() {
    let site = live_site().await;
    // An empty value in a query is a value left out, which for a thread's panel is the thread's
    // directory: the root is named `/`.
    let status = git_status(&site, &site.vic, Some("/")).await.ok();
    assert!(status.available);
    assert_eq!(status.reason, None);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.ahead, Some(1));
    assert_eq!(status.changes.len(), 1);
    assert_eq!(status.changes[0].path, "web/a.txt");
    assert_eq!(status.changes[0].worktree, "M");
    // The root may be named in either way.
    assert!(
        git_status(&site, &site.ada, Some(""))
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn a_nested_repository_path_reaches_the_machine_and_traversal_does_not() {
    let site = live_site().await;
    let nested = git_status(&site, &site.ada, Some("web")).await.ok();
    assert!(nested.available);
    assert_eq!(
        nested.branch.as_deref(),
        Some("web"),
        "the machine received the selected directory"
    );
    assert_eq!(site.sent(CommandKind::GitStatus), 1);
    let outside = git_status(&site, &site.ada, Some("../x")).await.problem();
    assert_eq!(outside.code, ErrorCode::NotFound);
    assert_eq!(site.sent(CommandKind::GitStatus), 1);
}

#[tokio::test]
async fn a_thread_panel_requests_its_own_directory_and_keeps_its_requested_work_facts() {
    let site = live_site().await;
    let mut conn = site.world.conn().await;
    sqlx::query("UPDATE threads SET workspace_dir = 'web' WHERE node_id = $1")
        .bind(site.thread)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let status = git_status(&site, &site.ada, None).await.ok();
    assert_eq!(
        status.branch.as_deref(),
        Some("web"),
        "an omitted panel path uses the thread's directory"
    );
    let mut conn = site.world.conn().await;
    let observed: serde_json::Value =
        sqlx::query_scalar("SELECT work_observed FROM threads WHERE node_id = $1")
            .bind(site.thread)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let work: orch_api_contract::source::DirectoryWork = serde_json::from_value(observed).unwrap();
    assert_eq!(work.uncommitted, Some(true));
    assert_eq!(work.changed_files, Some(1));
    assert_eq!(work.unpushed, Some(true));
    assert!(!work.stale);
}

#[tokio::test]
async fn a_difference_can_be_read_by_bytes_without_fetching_more_than_the_bounded_prefix() {
    let site = live_site().await;
    *site.disk.diff.lock().unwrap() = Some(("0123456789".to_owned(), true));
    let query = EnvironmentGitDiffGetQuery {
        thread: Some(site.thread),
        path: Some("web".to_owned()),
        staged: None,
    };
    let part = site
        .app()
        .call(EnvironmentGitDiffGet, &[&site.environment])
        .token(&site.ada.token)
        .query(&query)
        .header("range", "bytes=2-5")
        .send_bytes()
        .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.raw.as_ref(), b"2345");
    assert_eq!(part.headers["content-range"], "bytes 2-5/10");
    assert_eq!(
        part.headers["etag"].to_str().unwrap(),
        format!("\"{}\"", orch_types::Hash::of(b"0123456789")),
        "a review anchor names the full bounded difference, not just this range"
    );
    assert_eq!(part.headers[TRUNCATED_HEADER], "1");
    let beyond = site
        .app()
        .call(EnvironmentGitDiffGet, &[&site.environment])
        .token(&site.ada.token)
        .query(&query)
        .header("range", "bytes=10-")
        .send_bytes()
        .await;
    assert_eq!(beyond.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(beyond.headers["content-range"], "bytes */10");
}

#[tokio::test]
async fn a_machine_that_is_away_has_no_repository_to_read_and_says_that_it_is_offline() {
    let site = live_site().await;
    site.power_off().await;
    let status = git_status(&site, &site.ada, None).await.ok();
    assert!(!status.available);
    assert_eq!(status.reason, Some(GitUnavailable::Offline));
    assert!(status.changes.is_empty());
    let diff = git_diff(&site, &site.ada, None).await.problem();
    assert_eq!(diff.code, ErrorCode::SourceOffline);
}

#[tokio::test]
async fn repositories_are_read_by_whoever_may_read_the_files() {
    let site = live_site().await;
    assert!(!git_status(&site, &site.eve, None).await.status.is_success());
    assert!(!git_diff(&site, &site.eve, None).await.status.is_success());
    assert_eq!(
        site.sent(CommandKind::GitStatus) + site.sent(CommandKind::GitDiff),
        0
    );
}

#[tokio::test]
async fn the_differences_come_as_plain_text_and_what_is_staged_can_be_asked_for() {
    let site = live_site().await;
    let unstaged = git_diff(&site, &site.vic, None).await;
    assert_eq!(unstaged.status, StatusCode::OK);
    assert!(
        unstaged.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    assert!(String::from_utf8_lossy(&unstaged.raw).ends_with("unstaged"));
    assert!(unstaged.headers.get(TRUNCATED_HEADER).is_none());
    let staged = git_diff(&site, &site.vic, Some(true)).await;
    assert!(String::from_utf8_lossy(&staged.raw).ends_with("staged"));
}

#[tokio::test]
async fn a_difference_longer_than_two_mebibytes_is_cut_and_says_so() {
    let site = live_site().await;
    let long = "é".repeat(usize::try_from(GIT_DIFF_MAX_BYTES).unwrap());
    *site.disk.diff.lock().unwrap() = Some((long, false));
    let cut = git_diff(&site, &site.ada, None).await;
    assert_eq!(cut.status, StatusCode::OK);
    assert!(cut.raw.len() <= usize::try_from(GIT_DIFF_MAX_BYTES).unwrap());
    assert!(
        std::str::from_utf8(&cut.raw).is_ok(),
        "cut at a character, not in the middle of one"
    );
    assert_eq!(cut.headers[TRUNCATED_HEADER], "1");

    // A machine that cut it itself says so as well.
    *site.disk.diff.lock().unwrap() = Some(("short".to_owned(), true));
    let said = git_diff(&site, &site.ada, None).await;
    assert_eq!(said.raw.as_ref(), b"short");
    assert_eq!(said.headers[TRUNCATED_HEADER], "1");
}

#[tokio::test]
async fn a_machine_that_fails_the_command_is_a_failure_of_the_system_and_not_a_state_of_the_repository()
 {
    let site = live_site().await;
    site.disk.refuse_next(
        CommandKind::GitStatus,
        CommandError::new(MachineCode::Internal, "git blew up"),
    );
    let refused = git_status(&site, &site.ada, None).await.problem();
    assert_eq!(refused.status, 500);
}
