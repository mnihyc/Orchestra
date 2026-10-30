//! The system's own rules in Management: every org and the two limits of a root org, the settings of the system and what locks, and the defaults every org starts from.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{Cast, assert_no_system_admin, cast, lines_of, refused_to};
use chrono::Duration;
use orch_api_contract::access::{
    PageChange, PageChangeRow, PageChangeWhole, PageKind, PermissionPage, RowProfile,
};
use orch_api_contract::admin::{
    AdminDefaultsGet, AdminDefaultsUpdate, AdminOrgList, AdminOrgListQuery, AdminOrgUpdate,
    AdminSettingDelete, AdminSettingDeleteQuery, AdminSettingList, AdminSettingSet,
    AdminSettingSetBody, OrgLimitsChange,
};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::documents::DeletedFilter;
use orch_api_contract::nodes::{Org, OrgType};
use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody};
use orch_api_contract::settings::{
    LockScope, NodeSettingList, NodeSettingSet, NodeSettingSetBody, ResolvedSetting, SettingScope,
};
use orch_types::Id;
use places::Who;
use serde_json::{Value, json};
use support::World;

async fn orgs(world: &World, by: &Who, query: &AdminOrgListQuery) -> Page<Org> {
    world
        .app
        .call(AdminOrgList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

fn names(page: &Page<Org>) -> Vec<&str> {
    page.items
        .iter()
        .map(|org| org.base.name.as_str())
        .collect()
}

async fn set_limits(
    world: &World,
    by: &Who,
    org: Id,
    closed: Option<bool>,
    flat: Option<bool>,
) -> orch_api_core::testing::Reply<Org> {
    world
        .app
        .call(AdminOrgUpdate, &[&org])
        .token(&by.token)
        .body(&OrgLimitsChange { closed, flat })
        .send()
        .await
}

#[tokio::test]
async fn every_org_of_the_system_is_listed_by_name_at_every_level() {
    let world = World::new().await;
    let c = cast(&world).await;
    places::make_department(&world, &c.boss, c.acme, "Research", "research").await;

    let all = orgs(&world, &c.root, &AdminOrgListQuery::default()).await;
    let narrowed = orgs(
        &world,
        &c.root,
        &AdminOrgListQuery {
            q: Some("RES".into()),
            ..AdminOrgListQuery::default()
        },
    )
    .await;

    // Team orgs first, then people's own, each by name.
    assert_eq!(names(&all), ["Acme", "Research", "ada", "boss", "root"]);
    assert_eq!(names(&narrowed), ["Research"]);
    let personal = orgs(
        &world,
        &c.root,
        &AdminOrgListQuery {
            org_kind: Some(OrgType::Personal),
            ..AdminOrgListQuery::default()
        },
    )
    .await;
    assert_eq!(names(&personal), ["ada", "boss", "root"]);
    let acme = all
        .items
        .iter()
        .find(|org| org.base.name == "Acme")
        .unwrap();
    assert_eq!(acme.org_kind, OrgType::Team);
    assert_eq!(acme.depth, 0);
    assert_eq!(acme.org_count, 1);
    let ada = all.items.iter().find(|org| org.base.name == "ada").unwrap();
    assert_eq!(ada.org_kind, OrgType::Personal);
    assert!(
        ada.closed && ada.flat,
        "a personal org is closed and flat as shipped"
    );
}

#[tokio::test]
async fn the_list_of_orgs_goes_on_from_the_page_before_until_every_org_was_shown_once() {
    let world = World::new().await;
    let c = cast(&world).await;
    places::make_department(&world, &c.boss, c.acme, "Research", "research").await;
    let mut shown: Vec<String> = Vec::new();
    let mut cursor = None;

    loop {
        let page = orgs(
            &world,
            &c.root,
            &AdminOrgListQuery {
                limit: Some(2),
                cursor: cursor.clone(),
                ..AdminOrgListQuery::default()
            },
        )
        .await;
        shown.extend(names(&page).into_iter().map(str::to_owned));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(shown, ["Acme", "Research", "ada", "boss", "root"]);
}

#[tokio::test]
async fn a_deleted_root_org_is_listed_apart_until_it_is_purged() {
    let world = World::new().await;
    let c = cast(&world).await;
    world
        .app
        .call(NodeDelete, &[&c.acme])
        .token(&c.boss.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
        .ok();
    let only_deleted = AdminOrgListQuery {
        deleted: Some(DeletedFilter::Only),
        ..AdminOrgListQuery::default()
    };

    let live = orgs(&world, &c.root, &AdminOrgListQuery::default()).await;
    let deleted = orgs(&world, &c.root, &only_deleted).await;

    assert!(
        !names(&live).contains(&"Acme"),
        "it is not among the live ones"
    );
    assert_eq!(names(&deleted), ["Acme"]);
    assert_eq!(deleted.items[0].slug, "acme");
    assert_eq!(deleted.items[0].base.owner.username, "boss");
    world.advance(Duration::days(31));
    let purged = orgs(&world, &c.root, &only_deleted).await;
    assert!(
        purged.items.is_empty(),
        "past the days it is kept, it is not listed"
    );
}

#[tokio::test]
async fn a_system_admin_closes_a_root_org_and_opens_it_again_and_each_change_leaves_a_line() {
    let world = World::new().await;
    let c = cast(&world).await;
    let before = orgs(&world, &c.root, &AdminOrgListQuery::default()).await;
    let acme_before = before
        .items
        .iter()
        .find(|org| org.base.id == c.acme)
        .unwrap();
    assert!(
        !acme_before.closed && !acme_before.flat,
        "a team org is made with neither"
    );

    let closed = set_limits(&world, &c.root, c.acme, Some(true), None)
        .await
        .ok();
    let both = set_limits(&world, &c.root, c.acme, Some(false), Some(true))
        .await
        .ok();

    assert!(closed.closed && !closed.flat);
    assert!(!both.closed && both.flat);
    assert_eq!(
        closed.member_count, acme_before.member_count,
        "closing removes nobody"
    );
    assert!(closed.base.revision > acme_before.base.revision);
    let lines = lines_of(&world, "org.limits_changed").await;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].actor_id, Some(c.root.id()));
    assert_eq!(lines[0].node_id, Some(c.acme));
    assert!(
        lines[0].summary.contains("closed Acme"),
        "{}",
        lines[0].summary
    );
    assert_eq!(
        lines[0].details["closed"],
        json!({ "was": false, "now": true })
    );
    assert_eq!(
        lines[1].details["flat"],
        json!({ "was": false, "now": true })
    );
}

#[tokio::test]
async fn a_limit_that_is_asked_for_as_it_stands_changes_nothing_and_leaves_no_line() {
    let world = World::new().await;
    let c = cast(&world).await;
    set_limits(&world, &c.root, c.acme, Some(true), None)
        .await
        .ok();

    let again = set_limits(&world, &c.root, c.acme, Some(true), None)
        .await
        .ok();
    let nothing = set_limits(&world, &c.root, c.acme, None, None).await.ok();

    assert!(again.closed && nothing.closed);
    assert_eq!(lines_of(&world, "org.limits_changed").await.len(), 1);
}

#[tokio::test]
async fn an_org_inside_another_has_no_limits_of_its_own() {
    let world = World::new().await;
    let c = cast(&world).await;
    let research = places::make_department(&world, &c.boss, c.acme, "Research", "research")
        .await
        .base
        .id;

    let refused = set_limits(&world, &c.root, research, Some(true), None)
        .await
        .problem();
    let nowhere = set_limits(&world, &c.root, orch_types::new_id(), Some(true), None)
        .await
        .problem();

    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], json!("id"));
    assert_eq!(nowhere.code, ErrorCode::NotFound);
}

async fn settings(world: &World, by: &Who) -> Vec<ResolvedSetting> {
    world
        .app
        .call(AdminSettingList, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
}

fn setting<'a>(all: &'a [ResolvedSetting], key: &str) -> &'a ResolvedSetting {
    all.iter().find(|each| each.key == key).unwrap()
}

async fn put(
    world: &World,
    by: &Who,
    key: &str,
    value: Value,
    locked: Option<bool>,
) -> orch_api_core::testing::Reply<ResolvedSetting> {
    world
        .app
        .call(AdminSettingSet, &[&key])
        .token(&by.token)
        .body(&AdminSettingSetBody {
            value,
            locked,
            expected_revision: None,
        })
        .send()
        .await
}

#[tokio::test]
async fn the_systems_settings_say_where_each_value_comes_from_and_a_written_one_is_the_systems() {
    let world = World::new().await;
    let c = cast(&world).await;

    let shipped = settings(&world, &c.root).await;
    let written = put(&world, &c.root, "hosts.full_percent", json!(75), None)
        .await
        .ok();
    let after = settings(&world, &c.root).await;

    let full = setting(&shipped, "hosts.full_percent");
    assert_eq!(full.value, Some(json!(90)));
    assert_eq!(full.source.scope, SettingScope::Default);
    assert!(full.own.is_none() && full.writable);
    assert!(
        shipped.iter().all(|each| each.key != "agent.model"),
        "a key that is not set at the system is not among the system's"
    );
    assert_eq!(written.value, Some(json!(75)));
    let own = setting(&after, "hosts.full_percent");
    assert_eq!(own.source.scope, SettingScope::System);
    assert_eq!(own.own.as_ref().unwrap().value, json!(75));
    assert!(!own.own.as_ref().unwrap().locked);
    assert_eq!(own.own.as_ref().unwrap().revision, 1);
}

#[tokio::test]
async fn a_setting_that_is_removed_falls_back_to_the_value_orch_ships() {
    let world = World::new().await;
    let c = cast(&world).await;
    put(&world, &c.root, "hosts.full_percent", json!(60), None)
        .await
        .ok();

    let back = world
        .app
        .call(AdminSettingDelete, &[&"hosts.full_percent"])
        .token(&c.root.token)
        .send()
        .await
        .ok();

    assert_eq!(back.value, Some(json!(90)));
    assert_eq!(back.source.scope, SettingScope::Default);
    assert!(back.own.is_none());
    let lines = lines_of(&world, "system.settings_updated").await;
    assert_eq!(lines.len(), 2, "the writing and the taking back");
    assert_eq!(lines[1].details["reset"], json!(true));
}

#[tokio::test]
async fn a_system_setting_save_and_undo_refuse_an_unreviewed_revision() {
    let world = World::new().await;
    let c = cast(&world).await;
    let key = "hosts.full_percent";
    let first = world
        .app
        .call(AdminSettingSet, &[&key])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: json!(75),
            locked: None,
            expected_revision: Some(0),
        })
        .send()
        .await
        .ok();
    let first_revision = first.own.as_ref().unwrap().revision;
    let current = world
        .app
        .call(AdminSettingSet, &[&key])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: json!(80),
            locked: None,
            expected_revision: Some(first_revision),
        })
        .send()
        .await
        .ok();
    let current_revision = current.own.as_ref().unwrap().revision;
    let audit_count = lines_of(&world, "system.settings_updated").await.len();

    // Even the same value is not an acknowledged save at an older revision.
    let stale = world
        .app
        .call(AdminSettingSet, &[&key])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: json!(80),
            locked: None,
            expected_revision: Some(first_revision),
        })
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let body = serde_json::to_value(&stale).unwrap();
    assert_eq!(body["current"]["own"]["revision"], json!(current_revision));
    assert_eq!(body["current"]["value"], json!(80));

    let refused_undo = world
        .app
        .call(AdminSettingDelete, &[&key])
        .token(&c.root.token)
        .query(&AdminSettingDeleteQuery {
            expected_revision: Some(first_revision),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused_undo.code, ErrorCode::ConflictStaleRevision);
    let after = settings(&world, &c.root).await;
    assert_eq!(setting(&after, key).value, Some(json!(80)));
    assert_eq!(
        lines_of(&world, "system.settings_updated").await.len(),
        audit_count
    );

    let undone = world
        .app
        .call(AdminSettingDelete, &[&key])
        .token(&c.root.token)
        .query(&AdminSettingDeleteQuery {
            expected_revision: Some(current_revision),
        })
        .send()
        .await
        .ok();
    assert!(undone.own.is_none());
    assert_eq!(undone.value, Some(json!(90)));
}

#[tokio::test]
async fn a_setting_that_is_locked_decides_for_every_org_below_until_the_lock_is_lifted() {
    let world = World::new().await;
    let c = cast(&world).await;
    let at_acme = |key: &'static str| {
        let world = &world;
        let boss = &c.boss;
        let acme = c.acme;
        async move {
            let all: Vec<ResolvedSetting> = world
                .app
                .call(NodeSettingList, &[&acme])
                .token(&boss.token)
                .send()
                .await
                .ok();
            all.into_iter().find(|each| each.key == key).unwrap()
        }
    };
    assert_eq!(
        at_acme("invitations.max_hours").await.value,
        Some(json!(720))
    );

    put(
        &world,
        &c.root,
        "invitations.max_hours",
        json!(24),
        Some(true),
    )
    .await
    .ok();

    let locked = at_acme("invitations.max_hours").await;
    assert_eq!(locked.value, Some(json!(24)));
    assert_eq!(locked.locked_by.as_ref().unwrap().scope, LockScope::System);
    assert!(!locked.writable);
    let refused = world
        .app
        .call(NodeSettingSet, &[&c.acme, &"invitations.max_hours"])
        .token(&c.boss.token)
        .body(&NodeSettingSetBody {
            value: json!(48),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AccessLocked);

    put(
        &world,
        &c.root,
        "invitations.max_hours",
        json!(24),
        Some(false),
    )
    .await
    .ok();
    world
        .app
        .call(NodeSettingSet, &[&c.acme, &"invitations.max_hours"])
        .token(&c.boss.token)
        .body(&NodeSettingSetBody {
            value: json!(48),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        at_acme("invitations.max_hours").await.value,
        Some(json!(48))
    );
}

#[tokio::test]
async fn a_value_that_does_not_fit_and_a_key_that_is_not_the_systems_are_refused() {
    let world = World::new().await;
    let c = cast(&world).await;

    let too_large = put(&world, &c.root, "hosts.full_percent", json!(101), None)
        .await
        .problem();
    let unknown = put(&world, &c.root, "no.such.key", json!(1), None)
        .await
        .problem();
    let not_here = put(&world, &c.root, "agent.model", json!("m"), None)
        .await
        .problem();
    let nothing_to_lock = put(&world, &c.root, "hosts.full_percent", json!(80), Some(true))
        .await
        .problem();

    assert_eq!(too_large.code, ErrorCode::RequestInvalid);
    assert_eq!(too_large.extra["errors"][0]["path"], json!("value"));
    assert_eq!(unknown.code, ErrorCode::NotFound);
    assert_eq!(not_here.code, ErrorCode::RequestInvalid);
    assert_eq!(not_here.extra["errors"][0]["path"], json!("key"));
    assert_eq!(nothing_to_lock.code, ErrorCode::RequestInvalid);
    assert_eq!(nothing_to_lock.extra["errors"][0]["path"], json!("locked"));
    assert!(lines_of(&world, "system.settings_updated").await.is_empty());
}

#[tokio::test]
async fn every_act_on_the_systems_settings_leaves_its_line_and_a_value_written_as_it_stands_leaves_none()
 {
    let world = World::new().await;
    let c = cast(&world).await;

    put(&world, &c.root, "switch.wait_minutes", json!(15), None)
        .await
        .ok();
    put(&world, &c.root, "switch.wait_minutes", json!(15), None)
        .await
        .ok();
    put(&world, &c.root, "switch.wait_minutes", json!(20), None)
        .await
        .ok();

    let lines = lines_of(&world, "system.settings_updated").await;
    assert_eq!(lines.len(), 2, "the second write changed nothing");
    assert!(lines.iter().all(|line| line.actor_id == Some(c.root.id())));
    assert_eq!(lines[0].details["key"], json!("switch.wait_minutes"));
    assert!(
        lines[1].details["was"].as_str().unwrap().starts_with("15"),
        "the line says what it was: {}",
        lines[1].details
    );
}

async fn defaults(world: &World, by: &Who) -> PermissionPage {
    world
        .app
        .call(AdminDefaultsGet, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
}

fn entries(page: &PermissionPage) -> usize {
    page.rows.iter().map(|row| row.entries.len()).sum()
}

async fn change_defaults(
    world: &World,
    by: &Who,
    change: PageChange,
) -> orch_api_core::testing::Reply<PermissionPage> {
    world
        .app
        .call(AdminDefaultsUpdate, &[])
        .token(&by.token)
        .body(&change)
        .send()
        .await
}

#[tokio::test]
async fn the_defaults_page_of_the_system_is_changed_and_put_back_to_what_orch_ships() {
    let world = World::new().await;
    let c = cast(&world).await;
    let shipped = orch_access_contract::pages::shipped().len();

    let page = defaults(&world, &c.root).await;
    assert_eq!(page.page, PageKind::Defaults);
    assert_eq!(page.object.kind, "system");
    assert_eq!(entries(&page), shipped, "it holds what Orch ships");
    let row = page
        .rows
        .iter()
        .find(|row| row.profile == RowProfile::Any && !row.entries.is_empty())
        .unwrap();
    let emptied = change_defaults(
        &world,
        &c.root,
        PageChange {
            rows: Some(vec![PageChangeRow {
                permission: row.permission.clone(),
                profile: None,
                inherit: None,
                locked: Some(true),
                add: None,
                update: None,
                remove: Some(row.entries.iter().map(|entry| entry.id).collect()),
            }]),
            ..PageChange::default()
        },
    )
    .await
    .ok();

    let on_a_page_that_moved_on = change_defaults(
        &world,
        &c.root,
        PageChange {
            whole: Some(PageChangeWhole::Reset),
            expected_revision: Some(page.revision),
            ..PageChange::default()
        },
    )
    .await
    .problem();
    assert_eq!(
        on_a_page_that_moved_on.code,
        ErrorCode::ConflictStaleRevision
    );
    assert_eq!(
        on_a_page_that_moved_on.extra["current"]["revision"],
        json!(emptied.revision),
        "the refusal brings the page as it is"
    );
    assert!(
        !lines_of(&world, "permission.row_changed").await.is_empty(),
        "a change of the page leaves its line"
    );

    let changed = emptied
        .rows
        .iter()
        .find(|each| each.permission == row.permission)
        .unwrap();
    assert!(changed.entries.is_empty());
    assert!(
        changed.locked.is_some(),
        "the row is locked for every org below"
    );
    assert!(entries(&emptied) < shipped);
    let reset = change_defaults(
        &world,
        &c.root,
        PageChange {
            whole: Some(PageChangeWhole::Reset),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    assert_eq!(entries(&reset), shipped, "what Orch ships is back");
    assert!(
        reset.rows.iter().all(|each| each.locked.is_none()),
        "and no lock stays"
    );
    assert_eq!(
        lines_of(&world, "permission.row_changed")
            .await
            .iter()
            .filter(|line| line.details.get("entries") == Some(&json!(shipped)))
            .count(),
        1,
        "the reset says how many entries it put back"
    );
}

#[tokio::test]
async fn somebody_who_is_no_system_admin_is_refused_the_systems_rules_and_so_is_an_orgs_admin() {
    let world = World::new().await;
    let c: Cast = cast(&world).await;

    for (who, name) in [(&c.boss, "an org's admin"), (&c.ada, "an ordinary person")] {
        let problems = [
            refused_to(&world, AdminOrgList, &[], who, None, "").await,
            refused_to(
                &world,
                AdminOrgUpdate,
                &[&c.acme],
                who,
                Some(json!({ "closed": true })),
                "",
            )
            .await,
            refused_to(&world, AdminSettingList, &[], who, None, "").await,
            refused_to(
                &world,
                AdminSettingSet,
                &[&"hosts.full_percent"],
                who,
                Some(json!({ "value": 50 })),
                "",
            )
            .await,
            refused_to(
                &world,
                AdminSettingDelete,
                &[&"hosts.full_percent"],
                who,
                None,
                "",
            )
            .await,
            refused_to(&world, AdminDefaultsGet, &[], who, None, "").await,
            refused_to(
                &world,
                AdminDefaultsUpdate,
                &[],
                who,
                Some(json!({ "whole": "reset" })),
                "",
            )
            .await,
        ];
        for problem in &problems {
            assert_no_system_admin(problem, name);
        }
    }
    let acme = orgs(&world, &c.root, &AdminOrgListQuery::default()).await;
    let acme = acme.items.iter().find(|org| org.base.id == c.acme).unwrap();
    assert!(!acme.closed && !acme.flat, "nothing was changed");
    let all = settings(&world, &c.root).await;
    assert_eq!(
        setting(&all, "hosts.full_percent").value,
        Some(json!(90)),
        "nothing was written"
    );
}
