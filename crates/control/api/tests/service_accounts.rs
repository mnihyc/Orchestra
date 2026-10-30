//! The service identities of an org and their API tokens, over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use orch_api_contract::access::{
    NodeRoleCreate, NodeRoleList, Role, RoleCreateBody, SubjectInput, SubjectKind,
};
use orch_api_contract::auth::{ApiTokenCreateBody, ApiTokenInfo, TokenCreated};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::{
    OrgServiceAccountCreate, OrgServiceAccountCreateBody, OrgServiceAccountList, ServiceAccount,
    ServiceAccountDelete, ServiceAccountTokenCreate, ServiceAccountTokenDelete,
    ServiceAccountTokenList,
};
use orch_api_contract::nodes::{Node, NodeGet, NodeUpdate, NodeUpdateBody};
use orch_api_contract::users::UserKind;
use orch_api_core::testing::Reply;
use orch_types::{Id, Time};
use places::{Who, first_missing, join, make_org, make_project, missing_permissions, person};
use support::{Statements, World};

struct Setup {
    boss: Who,
    dan: Who,
    acme: Id,
    web: Id,
    docs: Id,
}

async fn setup(world: &World) -> Setup {
    let boss = person(world, "boss").await;
    let dan = person(world, "dan").await;
    let acme = make_org(world, &boss, "Acme", "acme").await;
    join(world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(world, &boss, acme.base.id, "Web").await;
    let docs = make_project(world, &boss, acme.base.id, "Docs").await;
    Setup {
        boss,
        dan,
        acme: acme.base.id,
        web: web.base.id,
        docs: docs.base.id,
    }
}

async fn make(
    world: &World,
    who: &Who,
    org: Id,
    username: &str,
    display_name: &str,
) -> Reply<ServiceAccount> {
    world
        .app
        .call(OrgServiceAccountCreate, &[&org])
        .token(&who.token)
        .body(&OrgServiceAccountCreateBody {
            username: username.to_owned(),
            display_name: display_name.to_owned(),
        })
        .send()
        .await
}

async fn list(world: &World, who: &Who, org: Id) -> Reply<Vec<ServiceAccount>> {
    world
        .app
        .call(OrgServiceAccountList, &[&org])
        .token(&who.token)
        .send()
        .await
}

fn named(name: &str) -> ApiTokenCreateBody {
    ApiTokenCreateBody {
        name: name.to_owned(),
        scopes: Some(vec!["*".to_owned()]),
        node_id: None,
        expires_at: None,
        proof: orch_api_contract::auth::SensitiveProof {
            password: Some(support::PASSWORD.to_owned()),
            ..Default::default()
        },
    }
}

async fn token(
    world: &World,
    who: &Who,
    service: Id,
    body: &ApiTokenCreateBody,
) -> Reply<TokenCreated> {
    world
        .app
        .call(ServiceAccountTokenCreate, &[&service])
        .token(&who.token)
        .body(body)
        .send()
        .await
}

async fn tokens(world: &World, who: &Who, service: Id) -> Reply<Vec<ApiTokenInfo>> {
    world
        .app
        .call(ServiceAccountTokenList, &[&service])
        .token(&who.token)
        .send()
        .await
}

/// Gives the service identity `service` a role in `place`, as `by`.
async fn give(world: &World, by: &Who, place: Id, service: Id, role: Role) {
    world
        .app
        .call(NodeRoleCreate, &[&place])
        .token(&by.token)
        .body(&RoleCreateBody {
            subject: SubjectInput {
                kind: SubjectKind::User,
                role: None,
                id: service,
            },
            role,
            expires_at: None,
            note: None,
        })
        .send()
        .await
        .ok();
}

async fn read_as(world: &World, secret: &str, node: Id) -> Reply<Node> {
    world.app.call(NodeGet, &[&node]).token(secret).send().await
}

#[tokio::test]
async fn a_service_identity_is_made_by_whoever_manages_the_members_and_listed_with_who_made_it() {
    let world = World::new().await;
    let s = setup(&world).await;
    let made = make(&world, &s.boss, s.acme, "ci-bot", "CI bot").await.ok();
    assert_eq!(made.user.kind, UserKind::Service);
    assert_eq!(
        (made.user.username.as_str(), made.user.display_name.as_str()),
        ("ci-bot", "CI bot")
    );
    assert_eq!(
        made.created_by.as_ref().map(|by| by.username.as_str()),
        Some("boss")
    );
    assert_eq!(made.token_count, 0);
    // A viewer makes nothing and lists nothing; the refusal says what is
    // lacking and who can give it.
    let refused = make(&world, &s.dan, s.acme, "dans-bot", "Dan's bot")
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&refused),
        vec!["members.manage".to_owned()]
    );
    assert_eq!(first_missing(&refused)["ask"][0]["username"], "boss");
    let refused = list(&world, &s.dan, s.acme).await.problem();
    assert_eq!(refused.extra["required"], "members.manage");
    assert_eq!(
        missing_permissions(&refused),
        vec!["members.manage".to_owned()]
    );
    let listed = list(&world, &s.boss, s.acme).await.ok();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].user.id, made.user.id);
    // An org that is not there keeps nothing.
    let nowhere = list(&world, &s.boss, orch_types::new_id()).await.problem();
    assert_eq!(nowhere.code, ErrorCode::NotFound);
    let audit = world.audit().await;
    assert!(
        audit
            .iter()
            .any(|action| action == "service_account.created")
    );
}

#[tokio::test]
async fn a_username_is_well_formed_and_free_among_people_and_the_short_names_of_root_orgs() {
    let world = World::new().await;
    let s = setup(&world).await;
    let too_long = "x".repeat(33);
    for bad in [
        "ab",
        "Bad",
        "has space",
        "-lead",
        "_lead",
        too_long.as_str(),
        "",
    ] {
        let refused = make(&world, &s.boss, s.acme, bad, "Bot").await.problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{bad:?}");
    }
    for bad in ["", "   "] {
        let refused = make(&world, &s.boss, s.acme, "fine-name", bad)
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{bad:?}");
    }
    let long_name = "n".repeat(65);
    let refused = make(&world, &s.boss, s.acme, "fine-name", &long_name)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    make(&world, &s.boss, s.acme, "ci-bot", "CI bot").await.ok();
    for (taken, why) in [
        ("ci-bot", "a service identity has it"),
        ("dan", "a person has it"),
        ("acme", "a root org has it as its short name"),
    ] {
        let refused = make(&world, &s.boss, s.acme, taken, "Bot").await.problem();
        assert_eq!(refused.code, ErrorCode::ConflictExists, "{why}");
    }
    // Nothing was made by the refusals.
    assert_eq!(list(&world, &s.boss, s.acme).await.ok().len(), 1);
}

#[tokio::test]
async fn a_token_is_shown_once_acts_by_the_roles_of_the_identity_and_is_listed_without_its_secret()
{
    let world = World::new().await;
    let s = setup(&world).await;
    let bot = make(&world, &s.boss, s.acme, "ci-bot", "CI bot").await.ok();
    // A member of nothing until it is given a role.
    let created = token(&world, &s.boss, bot.user.id, &named("deploy")).await;
    assert_eq!(
        created.headers.get("cache-control").map(|v| v.as_bytes()),
        Some(b"no-store".as_slice()),
        "a secret is not kept by caches"
    );
    let created = created.ok();
    assert!(created.token.as_deref().unwrap().starts_with("opat_"));
    assert_eq!(created.info.name, "deploy");
    assert_eq!(created.info.scopes, vec!["*".to_owned()]);
    assert!(created.info.node.is_none() && created.info.expires_at.is_none());
    let refused = read_as(&world, created.token.as_deref().unwrap(), s.web)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    give(&world, &s.boss, s.web, bot.user.id, Role::Developer).await;
    assert!(matches!(
        read_as(&world, created.token.as_deref().unwrap(), s.web)
            .await
            .ok(),
        Node::Project(_)
    ));
    let elsewhere = read_as(&world, created.token.as_deref().unwrap(), s.docs)
        .await
        .problem();
    assert_eq!(elsewhere.code, ErrorCode::AuthzDenied);
    // Listed by name, never with the secret; counted in the identity's list.
    let listed = tokens(&world, &s.boss, bot.user.id).await.ok();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.info.id);
    assert!(listed[0].last_used_at.is_some(), "it was used just now");
    assert!(
        !serde_json::to_string(&listed)
            .unwrap()
            .contains(created.token.as_deref().unwrap()),
        "the secret is not listed"
    );
    assert_eq!(list(&world, &s.boss, s.acme).await.ok()[0].token_count, 1);
    // A viewer sees none of this.
    let refused = tokens(&world, &s.dan, bot.user.id).await.problem();
    assert_eq!(refused.extra["required"], "members.manage");
    let refused = token(&world, &s.dan, bot.user.id, &named("mine"))
        .await
        .problem();
    assert_eq!(
        missing_permissions(&refused),
        vec!["members.manage".to_owned()]
    );
    let audit = world.audit().await;
    assert!(audit.iter().any(|action| action == "auth.token_created"));
}

#[tokio::test]
async fn a_token_is_kept_to_the_scopes_and_the_node_it_was_made_for_and_to_the_trees_of_its_org() {
    let world = World::new().await;
    let s = setup(&world).await;
    let bot = make(&world, &s.boss, s.acme, "ci-bot", "CI bot").await.ok();
    give(&world, &s.boss, s.acme, bot.user.id, Role::Admin).await;
    // Scoped to viewing a project: it reads Web and renames nothing, though
    // the identity is an admin.
    let viewing = token(
        &world,
        &s.boss,
        bot.user.id,
        &ApiTokenCreateBody {
            scopes: Some(vec!["project.view".to_owned()]),
            ..named("viewer")
        },
    )
    .await
    .ok();
    assert_eq!(viewing.info.scopes, vec!["project.view".to_owned()]);
    assert!(matches!(
        read_as(&world, viewing.token.as_deref().unwrap(), s.web)
            .await
            .ok(),
        Node::Project(_)
    ));
    let Node::Project(web) = places::read(&world, &s.boss, s.web).await else {
        panic!("a project is one");
    };
    let rename = |secret: &str, node: Id| {
        world
            .app
            .call(NodeUpdate, &[&node])
            .token(secret)
            .body(&NodeUpdateBody {
                readme_document_id: None,
                color: None,
                custom_color: None,
                name: Some("Website".to_owned()),
                description: None,
                expected_revision: web.base.revision,
            })
            .send()
    };
    assert_eq!(
        rename(viewing.token.as_deref().unwrap(), s.web)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    // Kept to Web: the other project is out of its reach.
    let kept = token(
        &world,
        &s.boss,
        bot.user.id,
        &ApiTokenCreateBody {
            node_id: Some(s.web),
            ..named("web only")
        },
    )
    .await
    .ok();
    assert_eq!(kept.info.node.as_ref().map(|node| node.id), Some(s.web));
    assert!(matches!(
        read_as(&world, kept.token.as_deref().unwrap(), s.web)
            .await
            .ok(),
        Node::Project(_)
    ));
    assert_eq!(
        read_as(&world, kept.token.as_deref().unwrap(), s.docs)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    // Not made: what the body asks for is not allowed.
    let beta = make_org(&world, &s.boss, "Beta", "beta").await;
    let beta_project = make_project(&world, &s.boss, beta.base.id, "Elsewhere").await;
    let past = Time::new(world.now() - Duration::hours(1));
    let cases: Vec<(&str, ApiTokenCreateBody)> = vec![
        ("no name", named("  ")),
        ("a name of 81 characters", named(&"n".repeat(81))),
        (
            "a scope nobody knows",
            ApiTokenCreateBody {
                scopes: Some(vec!["teleport".to_owned()]),
                ..named("odd")
            },
        ),
        (
            "an end in the past",
            ApiTokenCreateBody {
                expires_at: Some(past),
                ..named("late")
            },
        ),
        (
            "a node of another tree",
            ApiTokenCreateBody {
                node_id: Some(beta_project.base.id),
                ..named("abroad")
            },
        ),
    ];
    for (why, body) in cases {
        let refused = token(&world, &s.boss, bot.user.id, &body).await.problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid, "{why}");
    }
    assert_eq!(tokens(&world, &s.boss, bot.user.id).await.ok().len(), 2);
    // A node that is not there is not found.
    let nowhere = token(
        &world,
        &s.boss,
        bot.user.id,
        &ApiTokenCreateBody {
            node_id: Some(orch_types::new_id()),
            ..named("nowhere")
        },
    )
    .await
    .problem();
    assert_eq!(nowhere.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_token_stops_working_at_its_end_and_when_it_is_revoked() {
    let world = World::new().await;
    let s = setup(&world).await;
    let bot = make(&world, &s.boss, s.acme, "ci-bot", "CI bot").await.ok();
    let other = make(&world, &s.boss, s.acme, "other-bot", "Other bot")
        .await
        .ok();
    give(&world, &s.boss, s.acme, bot.user.id, Role::Viewer).await;
    let short = token(
        &world,
        &s.boss,
        bot.user.id,
        &ApiTokenCreateBody {
            expires_at: Some(Time::new(world.now() + Duration::hours(1))),
            ..named("short")
        },
    )
    .await
    .ok();
    let long = token(&world, &s.boss, bot.user.id, &named("long"))
        .await
        .ok();
    let foreign = token(&world, &s.boss, other.user.id, &named("foreign"))
        .await
        .ok();
    assert!(
        read_as(&world, short.token.as_deref().unwrap(), s.web)
            .await
            .body
            .is_ok()
    );
    world.advance(Duration::hours(2));
    let expired = read_as(&world, short.token.as_deref().unwrap(), s.web).await;
    assert_eq!(expired.status, 401);
    assert_eq!(expired.problem().code, ErrorCode::AuthInvalidToken);
    // Revoked by a manager of the members; not by a viewer.
    let revoke = |who: &Who, service: Id, id: Id| {
        world
            .app
            .call(ServiceAccountTokenDelete, &[&service, &id])
            .token(&who.token)
            .send()
    };
    let refused = revoke(&s.dan, bot.user.id, long.info.id).await.problem();
    assert_eq!(refused.extra["required"], "members.manage");
    assert!(
        read_as(&world, long.token.as_deref().unwrap(), s.web)
            .await
            .body
            .is_ok()
    );
    // Another identity's token is not this one's to revoke.
    let mixed = revoke(&s.boss, bot.user.id, foreign.info.id)
        .await
        .problem();
    assert_eq!(mixed.code, ErrorCode::NotFound);
    let still = tokens(&world, &s.boss, other.user.id).await.ok();
    assert_eq!(still.len(), 1, "the other identity keeps its token");
    assert_eq!(still[0].id, foreign.info.id);
    let gone = revoke(&s.boss, bot.user.id, long.info.id).await;
    assert_eq!(gone.status, 204);
    let refused = read_as(&world, long.token.as_deref().unwrap(), s.web).await;
    assert_eq!(refused.status, 401);
    assert_eq!(refused.problem().code, ErrorCode::AuthInvalidToken);
    let again = revoke(&s.boss, bot.user.id, long.info.id).await.problem();
    assert_eq!(again.code, ErrorCode::NotFound);
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "auth.token_revoked")
    );
}

#[tokio::test]
async fn deleting_a_service_identity_ends_its_tokens_its_roles_and_the_entries_that_name_it() {
    let world = World::new().await;
    let s = setup(&world).await;
    let bot = make(&world, &s.boss, s.acme, "ci-bot", "CI bot").await.ok();
    give(&world, &s.boss, s.web, bot.user.id, Role::Developer).await;
    let secret = token(&world, &s.boss, bot.user.id, &named("deploy"))
        .await
        .ok();
    assert!(
        read_as(&world, secret.token.as_deref().unwrap(), s.web)
            .await
            .body
            .is_ok()
    );
    let live_entries = |id: Id| {
        let world = &world;
        async move {
            let mut conn = world.conn().await;
            let named: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM permission_entries
                  WHERE holder_kind = 'user' AND holder_id = $1 AND removed_at IS NULL",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            named
        }
    };
    // Named on a page as well as given a role.
    {
        let mut conn = world.conn().await;
        sqlx::query(
            "INSERT INTO permission_entries (id, object_kind, object_id, page, permission, holder_kind, holder_id, created_by)
             VALUES ($1, 'node', $2, 'live', 'project.view', 'user', $3, $4)",
        )
        .bind(orch_types::new_id())
        .bind(s.docs)
        .bind(bot.user.id)
        .bind(s.boss.id())
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    assert_eq!(live_entries(bot.user.id).await, 1);
    // A viewer deletes nothing, and another identity's id is no identity of a person.
    let refused = world
        .app
        .call(ServiceAccountDelete, &[&bot.user.id])
        .token(&s.dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "members.manage");
    let a_person = world
        .app
        .call(ServiceAccountDelete, &[&s.dan.id()])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(
        a_person.code,
        ErrorCode::NotFound,
        "a person is no service identity"
    );
    let gone = world
        .app
        .call(ServiceAccountDelete, &[&bot.user.id])
        .token(&s.boss.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    // The token ends, the role is gone, the entry is gone, and so is the identity.
    let refused = read_as(&world, secret.token.as_deref().unwrap(), s.web).await;
    assert_eq!(refused.status, 401);
    assert_eq!(live_entries(bot.user.id).await, 0);
    let roles = world
        .app
        .call(NodeRoleList, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(roles.own.is_empty(), "{:?}", roles.own);
    assert!(list(&world, &s.boss, s.acme).await.ok().is_empty());
    for call in [
        tokens(&world, &s.boss, bot.user.id).await.problem().code,
        token(&world, &s.boss, bot.user.id, &named("late"))
            .await
            .problem()
            .code,
    ] {
        assert_eq!(call, ErrorCode::NotFound);
    }
    let again = world
        .app
        .call(ServiceAccountDelete, &[&bot.user.id])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::NotFound);
    // The name stays taken: nobody else is mistaken for it later.
    let reuse = make(&world, &s.boss, s.acme, "ci-bot", "CI bot")
        .await
        .problem();
    assert_eq!(reuse.code, ErrorCode::ConflictExists);
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "service_account.deleted")
    );
}

#[tokio::test]
async fn the_lists_of_service_identities_and_their_tokens_cost_the_same_statements_whatever_their_length()
 {
    let world = World::new().await;
    let s = setup(&world).await;
    let first = make(&world, &s.boss, s.acme, "bot-one", "Bot one")
        .await
        .ok();
    token(&world, &s.boss, first.user.id, &named("one"))
        .await
        .ok();
    async fn measure(
        world: &World,
        who: &Who,
        org: Id,
        service: Id,
    ) -> (usize, usize, usize, usize) {
        let before = Statements::now();
        let services = list(world, who, org).await.ok();
        let mid = Statements::now();
        let kept = tokens(world, who, service).await.ok();
        let after = Statements::now();
        (mid - before, after - mid, services.len(), kept.len())
    }
    let (few_services, few_tokens, a, b) = measure(&world, &s.boss, s.acme, first.user.id).await;
    assert_eq!((a, b), (1, 1));
    for index in 0..5 {
        let more = make(&world, &s.boss, s.acme, &format!("bot-{index}-x"), "More")
            .await
            .ok();
        token(&world, &s.boss, more.user.id, &named("more"))
            .await
            .ok();
        token(
            &world,
            &s.boss,
            first.user.id,
            &named(&format!("again {index}")),
        )
        .await
        .ok();
    }
    let (many_services, many_tokens, a, b) = measure(&world, &s.boss, s.acme, first.user.id).await;
    assert_eq!((a, b), (6, 6));
    assert_eq!(
        few_services, many_services,
        "{few_services} statements for 1 identity, {many_services} for 6"
    );
    assert_eq!(
        few_tokens, many_tokens,
        "{few_tokens} statements for 1 token, {many_tokens} for 6"
    );
}
