//! Finding people, and invitations: made within what the root org allows, looked up, accepted, listed and taken back.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use chrono::Duration;
use orch_api_contract::access::{PermissionPageGet, Role};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{Node, NodeGet};
use orch_api_contract::users::{
    InvitationAccept, InvitationAcceptBody, InvitationCreate, InvitationCreateBody,
    InvitationCreateReply, InvitationDelete, InvitationForm, InvitationKind, InvitationList,
    InvitationListQuery, InvitationLookup, UserGet, UserSearch, UserSearchQuery,
};
use orch_settings::Target;
use orch_types::Id;
use places::{Who, join, make_org, make_project, person};
use serde_json::json;
use support::World;

struct Setup {
    boss: Who,
    dan: Who,
    acme: Id,
    web: Id,
}

async fn setup(world: &World) -> Setup {
    let boss = person(world, "boss").await;
    let dan = person(world, "dan").await;
    let acme = make_org(world, &boss, "Acme", "acme").await;
    join(world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(world, &boss, acme.base.id, "Web").await;
    Setup {
        boss,
        dan,
        acme: acme.base.id,
        web: web.base.id,
    }
}

fn org_invitation(org: Id) -> InvitationCreateBody {
    InvitationCreateBody {
        kind: InvitationKind::Org,
        org_id: Some(org),
        node_id: None,
        chat_id: None,
        role: None,
        permissions: None,
        email: None,
        max_uses: None,
        expires_in_hours: None,
        note: None,
    }
}

async fn invite(
    world: &World,
    who: &Who,
    body: &InvitationCreateBody,
) -> orch_api_core::testing::Reply<InvitationCreateReply> {
    world
        .app
        .call(InvitationCreate, &[])
        .token(&who.token)
        .body(body)
        .send()
        .await
}

async fn accept(
    world: &World,
    who: &Who,
    token: &str,
) -> orch_api_core::testing::Reply<orch_api_contract::users::InvitationAcceptReply> {
    world
        .app
        .call(InvitationAccept, &[])
        .token(&who.token)
        .body(&InvitationAcceptBody {
            token: token.to_owned(),
        })
        .send()
        .await
}

#[tokio::test]
async fn people_are_found_among_those_who_share_an_org_and_by_exact_name_anywhere() {
    let world = World::new().await;
    let s = setup(&world).await;
    let alice = person(&world, "alice").await;
    join(&world, s.acme, s.acme, &alice, "viewer", &s.boss).await;
    let stranger = person(&world, "alicia").await;
    let search = |who: &Who, q: &str, org: Option<Id>| {
        world
            .app
            .call(UserSearch, &[])
            .token(&who.token)
            .query(&UserSearchQuery {
                q: q.to_owned(),
                org_id: org,
            })
            .send()
    };
    let names = |found: Vec<orch_api_contract::users::UserRef>| -> Vec<String> {
        found.into_iter().map(|user| user.username).collect()
    };
    // Dan shares Acme with Alice and not with Alicia.
    assert_eq!(names(search(&s.dan, "ali", None).await.ok()), vec!["alice"]);
    // Alicia is found by her exact username, and by her address.
    assert_eq!(
        names(search(&s.dan, "alicia", None).await.ok()),
        vec!["alicia"]
    );
    assert_eq!(
        names(search(&s.dan, "alicia@example.org", None).await.ok()),
        vec!["alicia"]
    );
    // Nobody is found by the start of a name outside the orgs they share.
    assert!(search(&stranger, "bo", None).await.ok().is_empty());
    // Too short.
    assert_eq!(
        search(&s.dan, "a", None).await.problem().code,
        ErrorCode::RequestInvalid
    );
    // Among the members of an org.
    assert_eq!(
        names(search(&s.dan, "bo", Some(s.acme)).await.ok()),
        vec!["boss"]
    );
    assert_eq!(
        search(&stranger, "bo", Some(s.acme)).await.problem().code,
        ErrorCode::AuthzDenied
    );
    // One person.
    let one = world
        .app
        .call(UserGet, &[&alice.id()])
        .token(&s.dan.token)
        .send()
        .await
        .ok();
    assert_eq!(one.username, "alice");
    assert_eq!(
        world
            .app
            .call(UserGet, &[&orch_types::new_id()])
            .token(&s.dan.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn an_org_invitation_brings_a_person_in_as_a_member_with_the_role_it_carries() {
    let world = World::new().await;
    let s = setup(&world).await;
    let eve = person(&world, "eve").await;
    let made = invite(&world, &s.boss, &org_invitation(s.acme)).await.ok();
    let delegated: Option<bool> =
        sqlx::query_scalar("SELECT created_as_admin FROM invitations WHERE id=$1")
            .bind(made.invitation.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        delegated,
        Some(false),
        "ordinary ownership does not fabricate administrator mode"
    );
    assert_eq!(made.invitation.kind, InvitationKind::Org);
    assert_eq!(made.invitation.form, InvitationForm::Link);
    assert_eq!(
        made.invitation.role,
        Some(Role::Viewer),
        "a viewer when left out"
    );
    assert_eq!((made.invitation.max_uses, made.invitation.uses), (1, 0));
    assert_eq!(made.invitation.org.as_ref().unwrap().id, s.acme);
    assert_eq!(
        made.invitation.expires_at.get() - world.now(),
        Duration::hours(168),
        "the policy's default life, a week"
    );
    assert!(made.token.starts_with("oinv_") && made.url.ends_with(&made.token));
    assert!(!made.sent);
    // A stranger cannot read the org; the invitation makes her a member.
    let accepted = accept(&world, &eve, &made.token).await.ok();
    let org = accepted.org.expect("the org she joined");
    assert_eq!(org.base.id, s.acme);
    assert_eq!(org.base.access.role, Some(Role::Viewer));
    assert!(matches!(
        world
            .app
            .call(NodeGet, &[&s.web])
            .token(&eve.token)
            .send()
            .await
            .ok(),
        Node::Project(_)
    ));
    // The invitation is used up, and she is a member already.
    let again = accept(&world, &eve, &made.token).await.problem();
    assert_eq!(again.code, ErrorCode::Gone);
    let second = invite(&world, &s.boss, &org_invitation(s.acme)).await.ok();
    let member = accept(&world, &eve, &second.token).await.problem();
    assert_eq!(member.code, ErrorCode::MemberExists);
    assert_eq!(member.extra["domain"]["id"], json!(s.acme));
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "invite.accepted")
    );
}

#[tokio::test]
async fn an_invitation_is_made_only_by_whoever_manages_the_members_and_within_what_the_policies_allow()
 {
    let world = World::new().await;
    let s = setup(&world).await;
    let refused = invite(&world, &s.dan, &org_invitation(s.acme))
        .await
        .problem();
    assert_eq!(refused.extra["required"], "members.manage");
    // A shared link serves up to the number the policy allows, and lasts as long.
    let mut many = org_invitation(s.acme);
    many.max_uses = Some(101);
    let problem = invite(&world, &s.boss, &many).await.problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    assert!(
        problem.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fault| {
                fault["path"] == "max_uses"
                    && fault["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("100"))
            }),
        "the max_uses field must report the policy's exact bound"
    );
    let mut long = org_invitation(s.acme);
    long.expires_in_hours = Some(721);
    let problem = invite(&world, &s.boss, &long).await.problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    assert!(
        problem.extra["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fault| {
                fault["path"] == "expires_in_hours"
                    && fault["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("720"))
            }),
        "the lifetime field must report the policy's exact bound"
    );
    // The root org leaves out the personal link.
    world
        .set_at(Target::Node(s.acme), "invitations.forms", json!(["link"]))
        .await;
    let mut personal = org_invitation(s.acme);
    personal.email = Some("new@example.org".to_owned());
    let problem = invite(&world, &s.boss, &personal).await.problem();
    assert_eq!(problem.code, ErrorCode::InvitationNotAllowed);
    assert_eq!(problem.extra["form"], "email");
    assert_eq!(problem.extra["scope"]["id"], json!(s.acme));
    // A tree that takes no member is closed to invitations.
    {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::testing::set_limits(&mut conn, s.acme, true, false)
            .await
            .unwrap();
    }
    let closed = invite(&world, &s.boss, &org_invitation(s.acme))
        .await
        .problem();
    assert_eq!(closed.code, ErrorCode::OrgClosed);
    // Admin is given by an owner or an admin of the org only.
    {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::testing::set_limits(&mut conn, s.acme, false, false)
            .await
            .unwrap();
    }
    let mut admin = org_invitation(s.acme);
    admin.role = Some(Role::Admin);
    assert!(invite(&world, &s.boss, &admin).await.body.is_ok());
}

#[tokio::test]
async fn an_invitation_is_looked_up_before_it_is_accepted_and_no_longer_once_it_has_ended() {
    let world = World::new().await;
    let s = setup(&world).await;
    let mut body = org_invitation(s.acme);
    body.max_uses = Some(2);
    body.role = Some(Role::Developer);
    let made = invite(&world, &s.boss, &body).await.ok();
    let preview = world
        .app
        .call(InvitationLookup, &[&made.token])
        .send()
        .await
        .ok();
    assert_eq!(preview.kind, InvitationKind::Org);
    assert_eq!(preview.org.as_ref().unwrap().slug, "acme");
    assert_eq!(preview.org.as_ref().unwrap().name, "Acme");
    assert_eq!(preview.role, Some(Role::Developer));
    assert_eq!(preview.inviter.username, "boss");
    assert!(!preview.email_bound);
    // Two people may use it, and not a third.
    for name in ["eve", "zed"] {
        let who = person(&world, name).await;
        accept(&world, &who, &made.token).await.ok();
    }
    let third = person(&world, "third").await;
    assert_eq!(
        accept(&world, &third, &made.token).await.problem().code,
        ErrorCode::Gone
    );
    assert_eq!(
        world
            .app
            .call(InvitationLookup, &[&made.token])
            .send()
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
    assert_eq!(
        world
            .app
            .call(InvitationLookup, &[&"oinv_nothing"])
            .send()
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
    // One that ran out of time.
    let late = invite(&world, &s.boss, &org_invitation(s.acme)).await.ok();
    world.advance(Duration::hours(169));
    assert_eq!(
        world
            .app
            .call(InvitationLookup, &[&late.token])
            .send()
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
}

#[tokio::test]
async fn a_personal_link_is_accepted_by_the_address_it_was_made_for_alone() {
    let world = World::new().await;
    let s = setup(&world).await;
    let eve = person(&world, "eve").await;
    let zed = person(&world, "zed").await;
    let mut body = org_invitation(s.acme);
    body.email = Some("  EVE@example.org ".to_owned());
    let made = invite(&world, &s.boss, &body).await.ok();
    assert_eq!(made.invitation.form, InvitationForm::Email);
    assert_eq!(made.invitation.email.as_deref(), Some("eve@example.org"));
    assert_eq!(made.invitation.max_uses, 1);
    let preview = world
        .app
        .call(InvitationLookup, &[&made.token])
        .send()
        .await
        .ok();
    assert!(preview.email_bound);
    // Another person with the link is refused, and the link is not used up.
    let refused = accept(&world, &zed, &made.token).await.problem();
    assert_eq!(refused.code, ErrorCode::RegistrationInviteRequired);
    accept(&world, &eve, &made.token).await.ok();
    // More than one use of a personal link is not asked for.
    let mut greedy = org_invitation(s.acme);
    greedy.email = Some("x@example.org".to_owned());
    greedy.max_uses = Some(5);
    assert_eq!(
        invite(&world, &s.boss, &greedy).await.problem().code,
        ErrorCode::RequestInvalid
    );
    let mut bad = org_invitation(s.acme);
    bad.email = Some("not-an-address".to_owned());
    assert_eq!(
        invite(&world, &s.boss, &bad).await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn a_mail_server_sends_a_personal_link_and_none_means_the_maker_hands_it_on() {
    let world = World::new().await;
    let s = setup(&world).await;
    let mut body = org_invitation(s.acme);
    body.email = Some("new@example.org".to_owned());
    let without = invite(&world, &s.boss, &body).await.ok();
    assert!(!without.sent);
    world.mail_server().await;
    body.email = Some("other@example.org".to_owned());
    let with = invite(&world, &s.boss, &body).await.ok();
    assert!(with.sent);
    let mails = world.mails("invitation", "other@example.org").await;
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0]["data"]["url"], json!(with.url));
}

#[tokio::test]
async fn a_node_invitation_names_a_person_on_one_place_and_makes_them_no_member() {
    let world = World::new().await;
    let s = setup(&world).await;
    let guest = person(&world, "guest").await;
    // Dan, a viewer, may not make one; the owner may.
    let body = InvitationCreateBody {
        kind: InvitationKind::Node,
        org_id: None,
        node_id: Some(s.web),
        chat_id: None,
        role: None,
        permissions: Some(vec!["project.view".to_owned()]),
        email: None,
        max_uses: None,
        expires_in_hours: None,
        note: Some("for the audit".to_owned()),
    };
    assert_eq!(
        invite(&world, &s.dan, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    let made = invite(&world, &s.boss, &body).await.ok();
    assert_eq!(made.invitation.node.as_ref().unwrap().id, s.web);
    assert_eq!(made.invitation.permissions, vec!["project.view".to_owned()]);
    let preview = world
        .app
        .call(InvitationLookup, &[&made.token])
        .send()
        .await
        .ok();
    assert_eq!(preview.what.as_ref().unwrap().name, "Web");
    let accepted = accept(&world, &guest, &made.token).await.ok();
    assert!(accepted.org.is_none());
    // She reads the project, is named on its page, and is nobody's member.
    assert!(matches!(
        world
            .app
            .call(NodeGet, &[&s.web])
            .token(&guest.token)
            .send()
            .await
            .ok(),
        Node::Project(_)
    ));
    let page = world
        .app
        .call(PermissionPageGet, &[&"nodes", &s.web])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    let view = page
        .rows
        .iter()
        .find(|row| row.permission == "project.view")
        .unwrap();
    assert!(view.entries.iter().any(|entry| entry.outside));
    let refused = world
        .app
        .call(NodeGet, &[&s.acme])
        .token(&guest.token)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "she was let into one place and nothing else"
    );
    // A permission the place does not have is refused.
    let mut odd = body.clone();
    odd.permissions = Some(vec!["provider.use".to_owned()]);
    assert_eq!(
        invite(&world, &s.boss, &odd).await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn the_system_invites_through_its_admins_alone() {
    let world = World::new().await;
    let s = setup(&world).await;
    let root = places::admin(&world, "root").await;
    let body = InvitationCreateBody {
        kind: InvitationKind::System,
        org_id: None,
        node_id: None,
        chat_id: None,
        role: None,
        permissions: None,
        email: None,
        max_uses: Some(3),
        expires_in_hours: None,
        note: None,
    };
    assert_eq!(
        invite(&world, &s.boss, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    let made = invite(&world, &root, &body).await.ok();
    assert_eq!(made.invitation.kind, InvitationKind::System);
    assert_eq!(made.invitation.max_uses, 3);
    let preview = world
        .app
        .call(InvitationLookup, &[&made.token])
        .send()
        .await
        .ok();
    assert_eq!(preview.kind, InvitationKind::System);
    let chat = InvitationCreateBody {
        kind: InvitationKind::Chat,
        chat_id: Some(orch_types::new_id()),
        ..body.clone()
    };
    // Chat invitations are supported. A missing target is distinct from a
    // malformed request that mixes an organization target with a chat target.
    assert_eq!(
        invite(&world, &root, &chat).await.problem().code,
        ErrorCode::NotFound
    );
    let malformed = InvitationCreateBody {
        org_id: Some(s.acme),
        ..chat
    };
    assert_eq!(
        invite(&world, &root, &malformed).await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn invitations_are_listed_for_whoever_manages_them_and_taken_back() {
    let world = World::new().await;
    let s = setup(&world).await;
    let first = invite(&world, &s.boss, &org_invitation(s.acme)).await.ok();
    world.advance(Duration::minutes(1));
    let second = invite(&world, &s.boss, &org_invitation(s.acme)).await.ok();
    let list = |who: &Who, org: Option<Id>| {
        world
            .app
            .call(InvitationList, &[])
            .token(&who.token)
            .query(&InvitationListQuery { org_id: org })
            .send()
    };
    let listed = list(&s.boss, Some(s.acme)).await.ok();
    assert_eq!(
        listed.iter().map(|each| each.id).collect::<Vec<_>>(),
        vec![second.invitation.id, first.invitation.id],
        "newest first"
    );
    assert_eq!(
        list(&s.dan, Some(s.acme)).await.problem().extra["required"],
        "members.manage"
    );
    // Without an org, what the caller made.
    assert_eq!(list(&s.boss, None).await.ok().len(), 2);
    assert!(list(&s.dan, None).await.ok().is_empty());
    // Taken back: it is marked, and no longer serves.
    let gone = world
        .app
        .call(InvitationDelete, &[&first.invitation.id])
        .token(&s.boss.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    let listed = list(&s.boss, Some(s.acme)).await.ok();
    assert!(
        listed
            .iter()
            .find(|each| each.id == first.invitation.id)
            .unwrap()
            .revoked
    );
    let eve = person(&world, "eve").await;
    assert_eq!(
        accept(&world, &eve, &first.token).await.problem().code,
        ErrorCode::Gone
    );
    let refused = world
        .app
        .call(InvitationDelete, &[&second.invitation.id])
        .token(&s.dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "members.manage");
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "invite.revoked")
    );
}

// Account admission, team membership and document shares are independent.
async fn register_from_invitation(
    world: &World,
    name: &str,
    link: &str,
) -> orch_api_core::testing::Reply<orch_api_contract::auth::AuthRegisterReply> {
    register_accepting_invitation(world, name, link, false).await
}

async fn register_accepting_invitation(
    world: &World,
    name: &str,
    link: &str,
    accept: bool,
) -> orch_api_core::testing::Reply<orch_api_contract::auth::AuthRegisterReply> {
    world
        .app
        .call(orch_api_contract::auth::AuthRegister, &[])
        .body(&orch_api_contract::auth::AuthRegisterBody {
            accept_invitation: accept,
            username: name.to_owned(),
            display_name: name.to_owned(),
            password: support::PASSWORD.to_owned(),
            email: None,
            invitation: Some(link.to_owned()),
            timezone: None,
        })
        .send()
        .await
}

fn system_invitation(uses: i64) -> InvitationCreateBody {
    InvitationCreateBody {
        kind: InvitationKind::System,
        org_id: None,
        node_id: None,
        chat_id: None,
        role: None,
        permissions: None,
        email: None,
        max_uses: Some(uses),
        expires_in_hours: None,
        note: None,
    }
}

#[tokio::test]
async fn an_admin_invited_account_can_receive_a_document_and_later_join_without_another_identity() {
    independent_account_document_and_team(true).await;
}

#[tokio::test]
async fn an_account_without_a_personal_org_can_receive_a_document_and_later_join_the_same_way() {
    independent_account_document_and_team(false).await;
}

async fn independent_account_document_and_team(personal: bool) {
    use orch_api_contract::access::{
        AccessChoice, Holder, HolderInput, PageChange, PageChangeName, PageChangeRow,
        PermissionPageUpdate,
    };
    use orch_api_contract::auth::AuthRegisterReply;
    use orch_api_contract::documents::{DocumentGet, NewDocumentAccess};
    let desk = desk::Desk::new().await;
    let mut body = desk::body("Already existing private document");
    body.access = Some(NewDocumentAccess::Private);
    let document = desk.make_body(&desk.ada, body).await.ok();
    let operator = places::admin(&desk.world, "operator").await;
    desk.world
        .set("orgs.personal_enabled", json!(personal))
        .await;
    let invitation = invite(&desk.world, &operator, &system_invitation(1))
        .await
        .ok();
    let AuthRegisterReply::Registered(orch_api_contract::auth::AuthRegisterResult {
        auth: account,
        ..
    }) = register_from_invitation(&desk.world, "newcomer", &invitation.token)
        .await
        .ok()
    else {
        panic!("the invitation must admit the account");
    };
    let person = account.user.user.id;
    let home: Option<Id> = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
        .bind(person)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        home.is_some(),
        personal,
        "account admission follows optional personal-home policy"
    );
    let memberships: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM org_members m JOIN orgs o ON o.node_id=m.org_id WHERE m.user_id=$1 AND o.org_kind='team'",
    ).bind(person).fetch_one(&desk.world.db.pool).await.unwrap();
    assert_eq!(memberships, 0, "system admission joins no team");
    assert!(!account.user.is_system_admin);
    let found = desk
        .world
        .app
        .call(UserSearch, &[])
        .token(&desk.ada.token)
        .query(&UserSearchQuery {
            q: "newcomer".to_owned(),
            org_id: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        found.iter().map(|user| user.id).collect::<Vec<_>>(),
        [person]
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&document.base.id])
            .token(&account.access_token)
            .send()
            .await
            .status,
        403
    );
    let shared = desk
        .world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: person }],
                choice: AccessChoice::FullView,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let read = desk
        .world
        .app
        .call(DocumentGet, &[&document.base.id])
        .token(&account.access_token)
        .send()
        .await
        .ok();
    assert_eq!(read.base.id, document.base.id);
    assert!(!read.access.owns);
    let listing = desk
        .world
        .app
        .call(orch_api_contract::nodes::SharedList, &[])
        .token(&account.access_token)
        .send()
        .await
        .ok();
    assert!(listing.items.iter().any(|item| matches!(item,
        orch_api_contract::nodes::SharedListItem::Document { document: reference, .. } if reference.id == document.base.id)),
        "the shared document must be discoverable without membership or a personal home");
    assert!(
        !read
            .access
            .permissions
            .iter()
            .any(|permission| permission == "doc.edit")
    );
    for node in [desk.org, desk.project, desk.topic] {
        assert_eq!(
            desk.world
                .app
                .call(NodeGet, &[&node])
                .token(&account.access_token)
                .send()
                .await
                .status,
            403,
            "a document share must not grant its surrounding place"
        );
    }
    let removed: Vec<_> = shared
        .rows
        .iter()
        .filter_map(|row| {
            let entries: Vec<_> = row
                .entries
                .iter()
                .filter(|entry| matches!(&entry.holder, Holder::User { user } if user.id==person))
                .map(|entry| entry.id)
                .collect();
            (!entries.is_empty()).then(|| PageChangeRow {
                permission: row.permission.clone(),
                profile: None,
                inherit: None,
                locked: None,
                add: None,
                update: None,
                remove: Some(entries),
            })
        })
        .collect();
    assert!(!removed.is_empty());
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &document.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            rows: Some(removed),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&document.base.id])
            .token(&account.access_token)
            .send()
            .await
            .status,
        403
    );
    let revoked = desk
        .world
        .app
        .call(orch_api_contract::nodes::SharedList, &[])
        .token(&account.access_token)
        .send()
        .await
        .ok();
    assert!(!revoked.items.iter().any(|item| matches!(item,
        orch_api_contract::nodes::SharedListItem::Document { document: reference, .. } if reference.id == document.base.id)),
        "taking back a share removes its discovery entrance too");
    let joining = invite(&desk.world, &desk.ada, &org_invitation(desk.org))
        .await
        .ok();
    let joined = desk
        .world
        .app
        .call(InvitationAccept, &[])
        .token(&account.access_token)
        .body(&InvitationAcceptBody {
            token: joining.token,
        })
        .send()
        .await
        .ok();
    assert_eq!(joined.org.unwrap().base.id, desk.org);
    let (same_home, count): (Option<Id>, i64) = sqlx::query_as(
        "SELECT personal_org_id,(SELECT count(*) FROM users WHERE username='newcomer') FROM users WHERE id=$1",
    ).bind(person).fetch_one(&desk.world.db.pool).await.unwrap();
    assert_eq!((same_home, count), (home, 1));
    let memberships: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM org_members m JOIN orgs o ON o.node_id=m.org_id WHERE m.user_id=$1 AND o.org_kind='team'",
    ).bind(person).fetch_one(&desk.world.db.pool).await.unwrap();
    assert_eq!(memberships, 1);
    assert_eq!(
        desk.world
            .app
            .call(NodeGet, &[&desk.project])
            .token(&account.access_token)
            .send()
            .await
            .status,
        200
    );
    assert_eq!(
        desk.world
            .app
            .call(DocumentGet, &[&document.base.id])
            .token(&account.access_token)
            .send()
            .await
            .status,
        403,
        "joining does not reopen the revoked private document share"
    );
}

#[tokio::test]
async fn registration_consumes_its_system_invitation_without_a_second_browser_request() {
    let world = World::new().await;
    let operator = places::admin(&world, "operator").await;
    let made = invite(&world, &operator, &system_invitation(1)).await.ok();
    let first = register_from_invitation(&world, "first-person", &made.token).await;
    assert_eq!(first.status, 201);
    let used: i32 = sqlx::query_scalar("SELECT uses FROM invitations WHERE id=$1")
        .bind(made.invitation.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        used, 1,
        "admission must consume capacity in its account-creation transaction"
    );
    let second = register_from_invitation(&world, "second-person", &made.token).await;
    assert_eq!(second.status, 403);
    let persons: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE username IN ('first-person','second-person')",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(persons, 1);
}

#[tokio::test]
async fn simultaneous_registration_cannot_reuse_the_last_invitation_place() {
    let world = World::new().await;
    let operator = places::admin(&world, "operator").await;
    let made = invite(&world, &operator, &system_invitation(1)).await.ok();
    let (a, b) = tokio::join!(
        register_from_invitation(&world, "first-person", &made.token),
        register_from_invitation(&world, "second-person", &made.token),
    );
    let mut statuses = [a.status, b.status];
    statuses.sort();
    assert_eq!(statuses, [201, 403]);
    let persons: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users WHERE username IN ('first-person','second-person')",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        persons, 1,
        "losing admission creates no account or personal organization"
    );
}

#[tokio::test]
async fn a_system_invitation_counts_an_existing_person_only_once() {
    let world = World::new().await;
    let operator = places::admin(&world, "operator").await;
    let recipient = person(&world, "recipient").await;
    let made = invite(&world, &operator, &system_invitation(3)).await.ok();
    accept(&world, &recipient, &made.token).await.ok();
    assert_eq!(accept(&world, &recipient, &made.token).await.status, 410);
    let (used, receipts): (i32,i64) = sqlx::query_as(
        "SELECT uses,(SELECT count(*) FROM invitation_uses WHERE invitation_id=$1) FROM invitations WHERE id=$1",
    ).bind(made.invitation.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((used, receipts), (1, 1));
}

#[tokio::test]
async fn withdrawn_system_issuers_cannot_admit_existing_or_new_accounts() {
    use orch_api_contract::admin::{AdminUserStatus, AdminUserUpdate, AdminUserUpdateBody};
    let mut outcomes = Vec::new();
    for suspend in [false, true] {
        let world = World::new().await;
        let issuer = places::admin(&world, "issuer").await;
        let keeper = places::admin(&world, "keeper").await;
        let recipient = person(&world, "recipient").await;
        let existing = invite(&world, &issuer, &system_invitation(1)).await.ok();
        let new_account = invite(&world, &issuer, &system_invitation(1)).await.ok();
        world
            .app
            .call(AdminUserUpdate, &[&issuer.id()])
            .token(&keeper.token)
            .body(&AdminUserUpdateBody {
                status: suspend.then_some(AdminUserStatus::Suspended),
                is_system_admin: (!suspend).then_some(false),
                acknowledge_admin_risk: Some(true),
            })
            .send()
            .await
            .ok();
        let accepted = accept(&world, &recipient, &existing.token).await;
        let registered =
            register_from_invitation(&world, "withdrawn-new", &new_account.token).await;
        let accounts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM users WHERE username='withdrawn-new'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        let used: i64 =
            sqlx::query_scalar("SELECT sum(uses)::bigint FROM invitations WHERE id=ANY($1)")
                .bind(vec![existing.invitation.id, new_account.invitation.id])
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        outcomes.push((
            accepted.status.as_u16(),
            registered.status.as_u16(),
            accounts,
            used,
        ));
    }
    assert_eq!(
        outcomes,
        [(410, 403, 0, 0), (410, 403, 0, 0)],
        "neither demoted nor suspended issuers may authorize a later admission"
    );
}

#[tokio::test]
async fn a_multi_use_system_link_cannot_register_or_admit_more_people_after_its_issuer_is_demoted()
{
    use orch_api_contract::admin::{AdminUserUpdate, AdminUserUpdateBody};
    let world = World::new().await;
    let issuer = places::admin(&world, "issuer").await;
    let keeper = places::admin(&world, "keeper").await;
    let recipient = person(&world, "recipient").await;
    world.set("orgs.personal_enabled", json!(true)).await;
    let invitation = invite(&world, &issuer, &system_invitation(3)).await.ok();
    register_from_invitation(&world, "first-person", &invitation.token)
        .await
        .ok();
    let homes_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orgs WHERE org_kind = 'personal'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    world
        .app
        .call(AdminUserUpdate, &[&issuer.id()])
        .token(&keeper.token)
        .body(&AdminUserUpdateBody {
            status: None,
            is_system_admin: Some(false),
            acknowledge_admin_risk: Some(true),
        })
        .send()
        .await
        .ok();
    let existing = accept(&world, &recipient, &invitation.token)
        .await
        .problem();
    assert_eq!(existing.code, ErrorCode::Gone);
    assert_eq!(existing.extra["reason"], "invitation_issuer_authority_lost");
    let preview = world
        .app
        .call(InvitationLookup, &[&invitation.token])
        .send()
        .await
        .problem();
    assert_eq!(preview.extra["reason"], "invitation_issuer_authority_lost");
    let registration = register_from_invitation(&world, "blocked-new", &invitation.token)
        .await
        .problem();
    assert_eq!(registration.code, ErrorCode::RegistrationInviteRequired);
    assert_eq!(
        registration.extra["reason"],
        "invitation_issuer_authority_lost"
    );
    let accounts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE username = 'blocked-new'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let homes_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orgs WHERE org_kind = 'personal'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let (uses, receipts): (i32, i64) = sqlx::query_as(
        "SELECT uses, (SELECT count(*) FROM invitation_uses WHERE invitation_id = $1) FROM invitations WHERE id = $1",
    ).bind(invitation.invitation.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        (accounts, homes_after, uses, receipts),
        (0, homes_before, 1, 1)
    );
}

#[tokio::test]
async fn target_invitation_registration_requires_explicit_acceptance_and_commits_one_membership() {
    use orch_api_contract::auth::AuthRegisterReply;
    let world = World::new().await;
    let s = setup(&world).await;
    let made = invite(&world, &s.boss, &org_invitation(s.acme)).await.ok();
    assert_eq!(
        register_from_invitation(&world, "invited-new", &made.token)
            .await
            .status,
        403
    );
    let absent: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE username='invited-new'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        absent, 0,
        "an org offer is not implicit standalone system admission"
    );
    let AuthRegisterReply::Registered(registered) =
        register_accepting_invitation(&world, "invited-new", &made.token, true)
            .await
            .ok()
    else {
        panic!("the explicit composition must register the person");
    };
    let person = registered.auth.user.user.id;
    assert!(!registered.auth.user.is_system_admin);
    assert_eq!(
        registered
            .accepted_invitation
            .as_ref()
            .and_then(|reply| reply.org.as_ref())
            .map(|org| org.base.id),
        Some(s.acme)
    );
    let joined: Vec<(Id,Id)> = sqlx::query_as(
        "SELECT m.org_id,m.domain_id FROM org_members m JOIN orgs o ON o.node_id=m.org_id WHERE m.user_id=$1 AND o.org_kind='team'",
    ).bind(person).fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(joined, [(s.acme, s.acme)]);
    let (used,receipts): (i32,i64) = sqlx::query_as(
        "SELECT uses,(SELECT count(*) FROM invitation_uses WHERE invitation_id=$1 AND user_id=$2) FROM invitations WHERE id=$1",
    ).bind(made.invitation.id).bind(person).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((used, receipts), (1, 1));
    assert_eq!(
        world
            .app
            .call(NodeGet, &[&s.web])
            .token(&registered.auth.access_token)
            .send()
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn open_registration_stays_independent_of_an_organization_offer_until_explicit_acceptance() {
    use orch_api_contract::auth::AuthRegisterReply;
    let world = World::new().await;
    let s = setup(&world).await;
    world.set("registration.mode", json!("open")).await;
    let made = invite(&world, &s.boss, &org_invitation(s.acme)).await.ok();
    let AuthRegisterReply::Registered(registered) =
        register_from_invitation(&world, "independent-new", &made.token)
            .await
            .ok()
    else {
        panic!("open standalone registration must succeed");
    };
    assert!(registered.accepted_invitation.is_none());
    let person = registered.auth.user.user.id;
    let memberships: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_members WHERE user_id=$1 AND org_id=$2")
            .bind(person)
            .bind(s.acme)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(memberships, 0);
    let used: i32 = sqlx::query_scalar("SELECT uses FROM invitations WHERE id=$1")
        .bind(made.invitation.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(used, 0);
    let joined = world
        .app
        .call(InvitationAccept, &[])
        .token(&registered.auth.access_token)
        .body(&InvitationAcceptBody { token: made.token })
        .send()
        .await
        .ok();
    assert_eq!(joined.org.unwrap().base.id, s.acme);
}

#[tokio::test]
async fn an_expired_composed_invitation_makes_no_account_while_open_standalone_registration_still_works()
 {
    use orch_api_contract::auth::AuthRegisterReply;
    let world = World::new().await;
    let s = setup(&world).await;
    world.set("registration.mode", json!("open")).await;
    let mut offer = org_invitation(s.acme);
    offer.expires_in_hours = Some(1);
    let made = invite(&world, &s.boss, &offer).await.ok();
    world.advance(Duration::hours(2));
    assert_eq!(
        register_accepting_invitation(&world, "expired-composed", &made.token, true)
            .await
            .status,
        403
    );
    let absent: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE username='expired-composed'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(absent, 0);
    let AuthRegisterReply::Registered(registered) =
        register_from_invitation(&world, "independent-open", &made.token)
            .await
            .ok()
    else {
        panic!("open registration needs no accepted target");
    };
    assert!(registered.accepted_invitation.is_none());
    let membership: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_members WHERE user_id=$1 AND org_id=$2")
            .bind(registered.auth.user.user.id)
            .bind(s.acme)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(membership, 0);
}

#[tokio::test]
async fn an_invitation_keeps_its_issuers_explicit_admin_mode_without_elevating_the_recipient() {
    use orch_api_contract::auth::{AdminModeSetBody, MeAdminModeGet, MeAdminModeSet, MeGet};
    use orch_api_contract::common::CSRF_HEADER;
    let world = World::new().await;
    let s = setup(&world).await;
    let issuer = world.person("operator").await;
    world.make_admin(&issuer).await;
    let signed = world.sign_in(&issuer).await;
    world
        .app
        .call(MeAdminModeSet, &[])
        .token(signed.token())
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    let recipient = person(&world, "recipient").await;
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_members WHERE user_id=$1 AND org_id=$2")
            .bind(issuer.id)
            .bind(s.acme)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        before, 0,
        "the administrator has no ordinary membership in this team"
    );
    let made = world
        .app
        .call(InvitationCreate, &[])
        .token(signed.token())
        .body(&org_invitation(s.acme))
        .send()
        .await
        .ok();
    let delegated: Option<bool> =
        sqlx::query_scalar("SELECT created_as_admin FROM invitations WHERE id=$1")
            .bind(made.invitation.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(delegated, Some(true));
    assert_eq!(
        world
            .app
            .call(orch_api_contract::auth::AuthLogout, &[])
            .header("cookie", &signed.browser())
            .header(CSRF_HEADER, "1")
            .send()
            .await
            .status,
        204,
        "an issued invitation has its own lifetime after the issuer signs out"
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(signed.token())
            .send()
            .await
            .status,
        401,
        "the actual invitation-creation session is revoked"
    );
    let accepted = accept(&world, &recipient, &made.token).await;
    assert_eq!(
        accepted.status.as_u16(),
        200,
        "acceptance uses the issuer's delegated mode, not the recipient's mode"
    );
    let joined = accepted.ok().org.unwrap();
    assert_eq!(joined.base.access.role, Some(Role::Viewer));
    let mode = world
        .app
        .call(MeAdminModeGet, &[])
        .token(&recipient.token)
        .send()
        .await
        .ok();
    assert!(!mode.as_admin);
    let me = world
        .app
        .call(MeGet, &[])
        .token(&recipient.token)
        .send()
        .await
        .ok();
    assert!(!me.is_system_admin);
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM org_members WHERE user_id=$1 AND org_id=$2")
            .bind(issuer.id)
            .bind(s.acme)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        after, 0,
        "delegation never adds the issuer as a team member"
    );
}
