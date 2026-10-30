//! People in Management: listed with the proofs strangers locked, made with a link, suspended and let in again with their sessions ended, made and unmade system admins, and given a reset link that works once.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{Cast, api_token, assert_no_system_admin, cast, lines_of, refused_to};
use chrono::Duration;
use orch_api_contract::admin::{
    AdminOrgList, AdminOrgListQuery, AdminUser, AdminUserCreate, AdminUserCreateBody,
    AdminUserCreateReply, AdminUserList, AdminUserListQuery, AdminUserResetLinkCreate,
    AdminUserResetLinkCreateBody, AdminUserResetLinkCreateReply, AdminUserStatus, AdminUserUpdate,
    AdminUserUpdateBody, OrgLimitsChange,
};
use orch_api_contract::auth::{AuthRecoveryConfirm, AuthRecoveryConfirmBody, MeGet, SignInProof};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::nodes::OrgType;
use orch_types::Id;
use places::Who;
use serde_json::json;
use support::{PASSWORD, World};

const NEW_PASSWORD: &str = "a new password, long enough";

async fn people(world: &World, by: &Who, query: &AdminUserListQuery) -> Page<AdminUser> {
    world
        .app
        .call(AdminUserList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

fn user<'a>(page: &'a Page<AdminUser>, name: &str) -> &'a AdminUser {
    page.items
        .iter()
        .find(|user| user.base.username == name)
        .unwrap()
}

fn names(page: &Page<AdminUser>) -> Vec<&str> {
    page.items
        .iter()
        .map(|user| user.base.username.as_str())
        .collect()
}

async fn update(
    world: &World,
    by: &Who,
    id: Id,
    body: &AdminUserUpdateBody,
) -> orch_api_core::testing::Reply<AdminUser> {
    world
        .app
        .call(AdminUserUpdate, &[&id])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

fn acknowledged(body: AdminUserUpdateBody) -> AdminUserUpdateBody {
    AdminUserUpdateBody {
        acknowledge_admin_risk: Some(true),
        ..body
    }
}

fn made(username: &str) -> AdminUserCreateBody {
    AdminUserCreateBody {
        username: username.to_owned(),
        display_name: username.to_owned(),
        email: None,
        is_system_admin: None,
        personal_org: None,
    }
}

async fn reset_link(
    world: &World,
    by: &Who,
    id: Id,
    clear_second_step: bool,
) -> AdminUserResetLinkCreateReply {
    world
        .app
        .call(AdminUserResetLinkCreate, &[&id])
        .token(&by.token)
        .body(&AdminUserResetLinkCreateBody {
            clear_second_step: Some(clear_second_step),
        })
        .send()
        .await
        .ok()
}

async fn use_link(world: &World, url: &str, password: &str) -> u16 {
    world
        .app
        .call(AuthRecoveryConfirm, &[])
        .body(&AuthRecoveryConfirmBody {
            token: url.rsplit('/').next().unwrap().to_owned(),
            password: password.to_owned(),
        })
        .send()
        .await
        .status
        .as_u16()
}

#[tokio::test]
async fn a_system_admin_lists_the_people_with_what_only_a_system_admin_reads_about_them() {
    let world = World::new().await;
    let c = cast(&world).await;

    let page = people(&world, &c.root, &AdminUserListQuery::default()).await;

    assert_eq!(names(&page), ["ada", "boss", "root"]);
    assert_eq!(page.next_cursor, None);
    let root = user(&page, "root");
    assert!(root.is_system_admin);
    assert_eq!(root.status, AdminUserStatus::Active);
    assert_eq!(root.email.as_deref(), Some("root@example.org"));
    assert!(root.email_verified);
    assert!(!root.totp_enabled && !root.email_code_second_step);
    assert!(root.locked.is_empty());
    assert!(!user(&page, "ada").is_system_admin);
}

#[tokio::test]
async fn the_list_of_people_is_narrowed_by_a_name_a_state_and_whether_they_administer_the_system() {
    let world = World::new().await;
    let c = cast(&world).await;
    let bea = world.person("bea").await;
    world.suspend(&bea).await;
    let (here, root) = (&world, &c.root);
    let ask = |query: AdminUserListQuery| async move { people(here, root, &query).await };

    let by_username = ask(AdminUserListQuery {
        q: Some("BOS".into()),
        ..AdminUserListQuery::default()
    })
    .await;
    let by_address = ask(AdminUserListQuery {
        q: Some("ADA@EXAMPLE".into()),
        ..AdminUserListQuery::default()
    })
    .await;
    let suspended = ask(AdminUserListQuery {
        status: Some(AdminUserStatus::Suspended),
        ..AdminUserListQuery::default()
    })
    .await;
    let admins = ask(AdminUserListQuery {
        system_admin: Some(true),
        ..AdminUserListQuery::default()
    })
    .await;
    let others = ask(AdminUserListQuery {
        system_admin: Some(false),
        ..AdminUserListQuery::default()
    })
    .await;

    assert_eq!(names(&by_username), ["boss"]);
    assert_eq!(names(&by_address), ["ada"]);
    assert_eq!(names(&suspended), ["bea"]);
    assert_eq!(names(&admins), ["root"]);
    assert_eq!(names(&others), ["ada", "bea", "boss"]);
}

#[tokio::test]
async fn the_list_of_people_goes_on_from_the_page_before_until_everybody_was_shown_once() {
    let world = World::new().await;
    let c = cast(&world).await;
    world.person("bea").await;
    let mut shown: Vec<String> = Vec::new();
    let mut cursor = None;
    let mut pages = 0;

    loop {
        let page = people(
            &world,
            &c.root,
            &AdminUserListQuery {
                limit: Some(2),
                cursor: cursor.clone(),
                ..AdminUserListQuery::default()
            },
        )
        .await;
        shown.extend(names(&page).into_iter().map(str::to_owned));
        pages += 1;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(shown, ["ada", "bea", "boss", "root"]);
    assert_eq!(pages, 2);
}

#[tokio::test]
async fn a_person_whom_strangers_locked_out_is_shown_with_the_proof_and_until_when() {
    let world = World::locking_after(3).await;
    let c = cast(&world).await;
    let started = world.now();
    for _ in 0..3 {
        let refused = world
            .password_login("ada", "not her password at all", "198.51.100.7", "")
            .await;
        assert!(refused.status.is_client_error());
    }

    let locked = people(&world, &c.root, &AdminUserListQuery::default()).await;
    let ada = user(&locked, "ada");
    let boss = user(&locked, "boss");

    assert_eq!(ada.locked.len(), 1);
    assert_eq!(ada.locked[0].way, SignInProof::Password);
    assert_eq!(
        ada.locked[0].until.get(),
        started + Duration::seconds(10_800),
        "locked until the first of the tries is three hours old"
    );
    assert!(boss.locked.is_empty(), "nobody else's name is locked");

    world.advance(Duration::seconds(10_801));
    let later = people(&world, &c.root, &AdminUserListQuery::default()).await;
    assert!(
        user(&later, "ada").locked.is_empty(),
        "the lock ends by itself"
    );
}

#[tokio::test]
async fn an_admin_makes_a_person_with_an_own_org_and_a_link_that_lets_them_set_a_password() {
    let world = World::new().await;
    let c = cast(&world).await;

    let reply: AdminUserCreateReply = world
        .app
        .call(AdminUserCreate, &[])
        .token(&c.root.token)
        .body(&AdminUserCreateBody {
            email: Some("dan@example.org".into()),
            display_name: "Dan Brown".into(),
            ..made("dan")
        })
        .send()
        .await
        .ok();

    assert_eq!(reply.user.base.username, "dan");
    assert_eq!(reply.user.base.display_name, "Dan Brown");
    assert_eq!(reply.user.status, AdminUserStatus::Active);
    assert!(!reply.user.is_system_admin);
    assert!(
        !reply.user.email_verified,
        "nobody showed yet that the address is theirs"
    );
    assert_eq!(
        world
            .password_login("dan", PASSWORD, "203.0.113.1", "")
            .await
            .status,
        401,
        "there is no password until the link is used"
    );
    assert_eq!(use_link(&world, &reply.reset_url, NEW_PASSWORD).await, 204);
    let signed = world
        .password_login("dan", NEW_PASSWORD, "203.0.113.1", "")
        .await;
    assert!(
        signed.status.is_success(),
        "they sign in with the password they set"
    );
    let orgs = world
        .app
        .call(AdminOrgList, &[])
        .token(&c.root.token)
        .query(&AdminOrgListQuery {
            q: Some("dan".into()),
            ..AdminOrgListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(orgs.items.len(), 1);
    assert_eq!(orgs.items[0].org_kind, OrgType::Personal);
    assert!(
        orgs.items[0].closed && orgs.items[0].flat,
        "closed and flat, as the system says for a new one"
    );
    let line = &lines_of(&world, "user.registered").await;
    let line = line
        .iter()
        .find(|line| line.object_id == Some(reply.user.base.id))
        .unwrap();
    assert_eq!(
        line.actor_id,
        Some(c.root.id()),
        "the trail names the admin who made them"
    );
    assert_eq!(line.details["by_admin"], json!(true));
}

#[tokio::test]
async fn a_new_persons_own_org_starts_as_the_admin_who_makes_them_says() {
    let world = World::new().await;
    let c = cast(&world).await;

    world
        .app
        .call(AdminUserCreate, &[])
        .token(&c.root.token)
        .body(&AdminUserCreateBody {
            personal_org: Some(OrgLimitsChange {
                closed: Some(false),
                flat: None,
            }),
            ..made("eve")
        })
        .send()
        .await
        .ok();

    let orgs = world
        .app
        .call(AdminOrgList, &[])
        .token(&c.root.token)
        .query(&AdminOrgListQuery {
            q: Some("eve".into()),
            ..AdminOrgListQuery::default()
        })
        .send()
        .await
        .ok();
    assert!(!orgs.items[0].closed, "open, as the admin said");
    assert!(orgs.items[0].flat, "flat, as the system's policy says");
}

#[tokio::test]
async fn a_person_who_is_made_a_system_admin_directly_leaves_the_line_of_it() {
    let world = World::new().await;
    let c = cast(&world).await;

    let reply = world
        .app
        .call(AdminUserCreate, &[])
        .token(&c.root.token)
        .body(&AdminUserCreateBody {
            is_system_admin: Some(true),
            ..made("dana")
        })
        .send()
        .await
        .ok();

    assert!(reply.user.is_system_admin);
    let granted = lines_of(&world, "user.admin_granted").await;
    assert_eq!(granted.len(), 1);
    assert_eq!(granted[0].object_id, Some(reply.user.base.id));
    assert_eq!(granted[0].actor_id, Some(c.root.id()));
}

#[tokio::test]
async fn a_username_that_is_taken_or_cannot_be_one_is_refused() {
    let world = World::new().await;
    let c = cast(&world).await;
    let (here, root) = (&world, &c.root);
    let make = |body: AdminUserCreateBody| async move {
        here.app
            .call(AdminUserCreate, &[])
            .token(&root.token)
            .body(&body)
            .send()
            .await
    };

    let taken = make(made("ada")).await.problem();
    let org_name = make(made("acme")).await.problem();
    let bad = make(made("Not Fine")).await.problem();
    let address = make(AdminUserCreateBody {
        email: Some("ada@example.org".into()),
        ..made("ada2")
    })
    .await
    .problem();

    assert_eq!(taken.code, ErrorCode::ConflictExists);
    assert_eq!(
        org_name.code,
        ErrorCode::ConflictExists,
        "usernames and the short names of root orgs are one namespace"
    );
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
    assert_eq!(bad.extra["errors"][0]["path"], json!("username"));
    assert_eq!(
        address.code,
        ErrorCode::ConflictExists,
        "an address belongs to one person"
    );
}

#[tokio::test]
async fn suspending_a_person_ends_their_sessions_and_tokens_at_once_and_lifting_it_brings_the_tokens_back()
 {
    let world = World::new().await;
    let c = cast(&world).await;
    let signed = world.sign_in(&c.ada.person).await;
    let token = api_token(&world, &c.ada).await;
    let me = |bearer: String| {
        let world = &world;
        async move { world.app.call(MeGet, &[]).token(&bearer).send().await }
    };
    assert!(me(signed.token().to_owned()).await.status.is_success());
    assert!(me(token.clone()).await.status.is_success());

    let suspended = update(
        &world,
        &c.root,
        c.ada.id(),
        &AdminUserUpdateBody {
            status: Some(AdminUserStatus::Suspended),
            ..AdminUserUpdateBody::default()
        },
    )
    .await
    .ok();

    assert_eq!(suspended.status, AdminUserStatus::Suspended);
    assert_eq!(
        me(signed.token().to_owned()).await.status,
        401,
        "the session is over"
    );
    assert_eq!(
        me(c.ada.token.clone()).await.status,
        401,
        "so is the one she was made with: every session ends, not the last"
    );
    let by_token = me(token.clone()).await.problem();
    assert_eq!(by_token.code, ErrorCode::AuthSuspended);
    let sign_in = world
        .password_login("ada", PASSWORD, "203.0.113.1", "")
        .await;
    assert!(!sign_in.status.is_success(), "they cannot sign in again");
    let lines = lines_of(&world, "user.suspended").await;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].actor_id, Some(c.root.id()));
    assert_eq!(lines[0].details["sessions_ended"], json!(2));

    let resumed = update(
        &world,
        &c.root,
        c.ada.id(),
        &AdminUserUpdateBody {
            status: Some(AdminUserStatus::Active),
            ..AdminUserUpdateBody::default()
        },
    )
    .await
    .ok();
    assert_eq!(resumed.status, AdminUserStatus::Active);
    assert!(me(token).await.status.is_success(), "the token works again");
    assert_eq!(
        me(signed.token().to_owned()).await.status,
        401,
        "the session that was ended stays ended"
    );
    assert_eq!(lines_of(&world, "user.resumed").await.len(), 1);
}

#[tokio::test]
async fn a_change_that_makes_a_system_admin_or_touches_one_waits_for_the_callers_acknowledgement() {
    let world = World::new().await;
    let c = cast(&world).await;
    let second = world.person("second").await;
    world.make_admin(&second).await;
    let suspend = AdminUserUpdateBody {
        status: Some(AdminUserStatus::Suspended),
        ..AdminUserUpdateBody::default()
    };
    let make_admin = AdminUserUpdateBody {
        is_system_admin: Some(true),
        ..AdminUserUpdateBody::default()
    };

    let touching = update(&world, &c.root, second.id, &suspend).await.problem();
    let making = update(&world, &c.root, c.ada.id(), &make_admin)
        .await
        .problem();
    let ordinary = update(&world, &c.root, c.boss.id(), &suspend).await;
    let touched = update(&world, &c.root, second.id, &acknowledged(suspend)).await;
    let made = update(&world, &c.root, c.ada.id(), &acknowledged(make_admin)).await;

    assert_eq!(touching.code, ErrorCode::AdminRiskNotAcknowledged);
    assert_eq!(making.code, ErrorCode::AdminRiskNotAcknowledged);
    assert!(
        ordinary.status.is_success(),
        "no acknowledgement for an ordinary person"
    );
    assert_eq!(touched.ok().status, AdminUserStatus::Suspended);
    assert!(made.ok().is_system_admin);
    assert_eq!(lines_of(&world, "user.admin_granted").await.len(), 1);
}

#[tokio::test]
async fn the_last_system_admin_who_can_sign_in_stays_one() {
    let world = World::new().await;
    let c = cast(&world).await;
    let demote = acknowledged(AdminUserUpdateBody {
        is_system_admin: Some(false),
        ..AdminUserUpdateBody::default()
    });
    let suspend = acknowledged(AdminUserUpdateBody {
        status: Some(AdminUserStatus::Suspended),
        ..AdminUserUpdateBody::default()
    });
    let both = acknowledged(AdminUserUpdateBody {
        status: Some(AdminUserStatus::Suspended),
        is_system_admin: Some(false),
        ..AdminUserUpdateBody::default()
    });

    let alone_demoted = update(&world, &c.root, c.root.id(), &demote)
        .await
        .problem();
    let alone_suspended = update(&world, &c.root, c.root.id(), &suspend)
        .await
        .problem();
    let alone_both = update(&world, &c.root, c.root.id(), &both).await.problem();

    for refused in [&alone_demoted, &alone_suspended, &alone_both] {
        assert_eq!(refused.code, ErrorCode::AdminLastAdmin);
        assert_eq!(refused.status, 409);
    }
    let still = people(&world, &c.root, &AdminUserListQuery::default()).await;
    let root = user(&still, "root");
    assert!(root.is_system_admin && root.status == AdminUserStatus::Active);

    // A second admin frees the first; a suspended one does not count.
    update(
        &world,
        &c.root,
        c.ada.id(),
        &acknowledged(AdminUserUpdateBody {
            is_system_admin: Some(true),
            ..AdminUserUpdateBody::default()
        }),
    )
    .await
    .ok();
    update(&world, &c.root, c.ada.id(), &suspend).await.ok();
    let still_alone = update(&world, &c.root, c.root.id(), &demote)
        .await
        .problem();
    assert_eq!(
        still_alone.code,
        ErrorCode::AdminLastAdmin,
        "a suspended admin cannot take over"
    );
    update(
        &world,
        &c.root,
        c.ada.id(),
        &AdminUserUpdateBody {
            status: Some(AdminUserStatus::Active),
            acknowledge_admin_risk: Some(true),
            ..AdminUserUpdateBody::default()
        },
    )
    .await
    .ok();
    let freed = update(&world, &c.root, c.root.id(), &demote).await.ok();
    assert!(!freed.is_system_admin);
}

#[tokio::test]
async fn an_unknown_person_and_a_service_account_are_nobody_to_change() {
    let world = World::new().await;
    let c = cast(&world).await;
    let body = AdminUserUpdateBody {
        status: Some(AdminUserStatus::Suspended),
        ..AdminUserUpdateBody::default()
    };
    let bot = world
        .app
        .call(
            orch_api_contract::directory::OrgServiceAccountCreate,
            &[&c.acme],
        )
        .token(&c.boss.token)
        .body(&orch_api_contract::directory::OrgServiceAccountCreateBody {
            username: "ci-bot".into(),
            display_name: "CI bot".into(),
        })
        .send()
        .await
        .ok();

    let nobody = update(&world, &c.root, orch_types::new_id(), &body)
        .await
        .problem();
    let a_bot = update(&world, &c.root, bot.user.id, &body).await.problem();
    let link = world
        .app
        .call(AdminUserResetLinkCreate, &[&bot.user.id])
        .token(&c.root.token)
        .body(&AdminUserResetLinkCreateBody::default())
        .send()
        .await
        .problem();

    assert_eq!(nobody.code, ErrorCode::NotFound);
    assert_eq!(a_bot.code, ErrorCode::NotFound);
    assert_eq!(link.code, ErrorCode::NotFound);
    let listed = people(&world, &c.root, &AdminUserListQuery::default()).await;
    assert!(
        !names(&listed).contains(&"ci-bot"),
        "a service account is no person"
    );
}

#[tokio::test]
async fn a_reset_link_sets_a_password_once_and_a_second_step_goes_only_when_the_link_says_so() {
    let world = World::new().await;
    let c = cast(&world).await;
    let bea = world.person("bea").await;
    for person in [&c.ada.person, &bea] {
        let signed = world.sign_in(person).await;
        world.authenticator(&signed).await;
    }

    let clearing = reset_link(&world, &c.root, c.ada.id(), true).await;
    let keeping = reset_link(&world, &c.root, bea.id, false).await;

    assert!(clearing.expires_at.get() > world.now());
    assert_eq!(use_link(&world, &clearing.url, NEW_PASSWORD).await, 204);
    assert!(
        use_link(&world, &clearing.url, "another password, long enough").await >= 400,
        "a link works once"
    );
    assert_eq!(use_link(&world, &keeping.url, NEW_PASSWORD).await, 204);
    let ada_in = world
        .password_login("ada", NEW_PASSWORD, "203.0.113.1", "")
        .await;
    assert!(
        ada_in.status.is_success(),
        "ada lost her second step with it"
    );
    let bea_in = world
        .password_login("bea", NEW_PASSWORD, "203.0.113.2", "")
        .await
        .problem();
    assert_eq!(
        bea_in.code,
        ErrorCode::AuthSecondStepRequired,
        "bea keeps hers"
    );
    assert_eq!(lines_of(&world, "auth.second_step_cleared").await.len(), 1);
}

#[tokio::test]
async fn a_reset_link_leaves_its_line_in_the_trail_naming_the_admin_who_made_it() {
    let world = World::new().await;
    let c = cast(&world).await;

    reset_link(&world, &c.root, c.ada.id(), true).await;

    let lines = lines_of(&world, "auth.password_reset").await;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].actor_id, Some(c.root.id()));
    assert_eq!(lines[0].object_id, Some(c.ada.id()));
    assert_eq!(lines[0].details["stage"], json!("link_made"));
    assert_eq!(lines[0].details["clear_second_step"], json!(true));
    assert!(
        !lines[0].details.to_string().contains("oem_"),
        "the secret of the link is in no line"
    );
}

#[tokio::test]
async fn somebody_who_is_no_system_admin_is_refused_every_request_about_people_and_so_is_an_orgs_admin()
 {
    let world = World::new().await;
    let c: Cast = cast(&world).await;
    let id = c.ada.id();

    for (who, name) in [(&c.boss, "an org's admin"), (&c.ada, "an ordinary person")] {
        let problems = [
            refused_to(&world, AdminUserList, &[], who, None, "").await,
            refused_to(
                &world,
                AdminUserCreate,
                &[],
                who,
                Some(json!({ "username": "zed", "display_name": "Zed" })),
                "",
            )
            .await,
            refused_to(&world, AdminUserUpdate, &[&id], who, Some(json!({})), "").await,
            refused_to(
                &world,
                AdminUserResetLinkCreate,
                &[&id],
                who,
                Some(json!({})),
                "",
            )
            .await,
        ];
        for problem in &problems {
            assert_no_system_admin(problem, name);
        }
    }
    let listed = people(&world, &c.root, &AdminUserListQuery::default()).await;
    assert!(!names(&listed).contains(&"zed"), "nothing was made");
}

#[tokio::test]
async fn a_system_admin_with_an_api_token_is_refused_because_these_are_done_in_a_browser() {
    let world = World::new().await;
    let c = cast(&world).await;
    let token = api_token(&world, &c.root).await;

    let list = world
        .app
        .call(AdminUserList, &[])
        .token(&token)
        .send_bytes()
        .await;
    let link = world
        .app
        .call(AdminUserResetLinkCreate, &[&c.ada.id()])
        .token(&token)
        .raw_body(&json!({}))
        .send_bytes()
        .await;

    for refused in [list, link] {
        assert_eq!(refused.problem().code, ErrorCode::AuthSessionRequired);
    }
    assert!(lines_of(&world, "auth.password_reset").await.is_empty());
}
