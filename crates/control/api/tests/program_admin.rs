//! System-admin authority, bounded program/build pages and declaration-based capability projections over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{assert_no_system_admin, cast, refused_to};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::program_admin::{
    AdminProgramBuildList, AdminProgramChannelsRead, AdminProgramGet, AdminProgramList,
    AdminProgramVersionFetch, AdminProgramVersionGet,
};
use serde_json::json;
use support::World;

#[tokio::test]
async fn program_pages_are_system_admin_only_and_cursors_cannot_cross_programs_or_filters() {
    let world = World::new().await;
    let c = cast(&world).await;
    for who in [&c.boss, &c.ada] {
        assert_no_system_admin(
            &refused_to(&world, AdminProgramList, &[], who, None, "").await,
            "program administration",
        );
        assert_no_system_admin(
            &refused_to(&world, AdminProgramGet, &[&"fake"], who, None, "").await,
            "program administration",
        );
        assert_no_system_admin(
            &refused_to(&world, AdminProgramBuildList, &[&"fake"], who, None, "").await,
            "program administration",
        );
    }
    for who in [&c.boss, &c.ada] {
        assert_no_system_admin(
            &refused_to(
                &world,
                AdminProgramVersionGet,
                &[&"fake", &"1.0.0"],
                who,
                None,
                "",
            )
            .await,
            "program administration",
        );
        assert_no_system_admin(
            &refused_to(
                &world,
                AdminProgramVersionFetch,
                &[&"fake", &"1.0.0"],
                who,
                None,
                "",
            )
            .await,
            "program administration",
        );
        assert_no_system_admin(
            &refused_to(&world, AdminProgramChannelsRead, &[&"fake"], who, None, "").await,
            "program administration",
        );
    }
    let first = world
        .app
        .call(AdminProgramList, &[])
        .token(&c.root.token)
        .raw_query("limit=1")
        .send()
        .await
        .ok();
    assert_eq!(first.items.len(), 1);
    let cursor = first.next_cursor.unwrap();
    let second = world
        .app
        .call(AdminProgramList, &[])
        .token(&c.root.token)
        .raw_query(&format!("limit=1&cursor={cursor}"))
        .send()
        .await
        .ok();
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].harness.kind, second.items[0].harness.kind);
    let crossed = world
        .app
        .call(AdminProgramBuildList, &[&"fake"])
        .token(&c.root.token)
        .raw_query(&format!("cursor={cursor}"))
        .send()
        .await
        .problem();
    assert_eq!(crossed.code, ErrorCode::RequestInvalid);
    let detail = world
        .app
        .call(AdminProgramGet, &[&"fake"])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert_eq!(detail.summary.harness.kind, "fake");
    assert!(detail.program.is_some());
    if detail.descriptor.is_none() {
        assert!(detail.capabilities.is_none())
    }
}

#[tokio::test]
async fn one_build_page_and_platform_summary_do_not_depend_on_loading_the_full_build_table() {
    let world = World::new().await;
    let c = cast(&world).await;
    sqlx::query("INSERT INTO settings(key,value) SELECT 'harness_build:external:1.0.'||g||':linux:x86_64',
        jsonb_build_object('kind','external','version','1.0.'||g,'os','linux','arch','x86_64','source','admin','checksum_origin','admin',
        'status',CASE WHEN g%2=0 THEN 'checked' ELSE 'failed' END,'is_default',g=3,'url','https://example.test/build','sha256',repeat('a',64)) FROM generate_series(0,99) g")
        .execute(&world.db.pool).await.unwrap();
    let page = world
        .app
        .call(AdminProgramBuildList, &[&"external"])
        .token(&c.root.token)
        .raw_query("limit=7")
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 7);
    assert_eq!(page.items[0].version, "1.0.99");
    assert!(page.next_cursor.is_some());
    assert!(page.items.iter().all(|b| b.carry.is_some()));
    let detail = world
        .app
        .call(AdminProgramGet, &[&"external"])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert_eq!(detail.summary.platforms.len(), 1);
    let platform = &detail.summary.platforms[0];
    assert_eq!(platform.builds, 100);
    assert_eq!(platform.newest.as_deref(), Some("1.0.99"));
    assert_eq!(platform.newest_checked.as_deref(), Some("1.0.98"));
    assert_eq!(platform.default_version.as_deref(), Some("1.0.3"));
    assert!(platform.shipped.is_none());
    assert_eq!(detail.versions, json!({}));
    assert!(detail.program.is_none());
    assert!(detail.capabilities.is_none());
    let exact = world
        .app
        .call(AdminProgramVersionGet, &[&"external", &"1.0.3"])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].version, "1.0.3");
    assert!(exact[0].is_default);
    let fetched = world
        .app
        .call(AdminProgramVersionFetch, &[&"external", &"1.0.3"])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].version, "1.0.3");
}
