//! The signed provider round trip keeps the exact acknowledgement and rechecks it at joining.
use super::*;
use crate::support::FreshInvitationCreateReply as InvitationCreateReply;
use orch_api_contract::nodes::{OrgCreate, OrgCreateBody};
use orch_api_contract::users::{
    InvitationCreate, InvitationCreateBody, InvitationKind, InvitationLookup, InvitationTerms,
};
use orch_settings::Target;

async fn invitation(sso: &Sso) -> (Id, InvitationCreateReply, InvitationTerms) {
    let root = sso
        .world
        .app
        .call(OrgCreate, &[])
        .token(sso.admin.token())
        .body(&OrgCreateBody {
            name: "Invited team".into(),
            slug: "invited-team".into(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    sso.world
        .set_at(
            Target::Node(root),
            "org.terms",
            json!("Read these before joining the team."),
        )
        .await;
    let offer = sso
        .world
        .app
        .call(InvitationCreate, &[])
        .token(sso.admin.token())
        .body(&InvitationCreateBody {
            kind: InvitationKind::Org,
            org_id: Some(root),
            node_id: None,
            chat_id: None,
            role: None,
            permissions: None,
            email: None,
            max_uses: None,
            expires_in_hours: None,
            note: None,
        })
        .send()
        .await
        .full_invitation()
        .ok();
    let terms = sso
        .world
        .app
        .call(InvitationLookup, &[&offer.token])
        .send()
        .await
        .ok()
        .terms
        .unwrap();
    (root, offer, terms)
}

async fn begin(
    sso: &Sso,
    browser: &mut Browser,
    offer: &InvitationCreateReply,
    hash: &str,
) -> String {
    let reply = sso
        .world
        .app
        .call(AuthOidcStart, &[&KEY])
        .header("cookie", &browser.header())
        .query(&AuthOidcStartQuery {
            redirect: Some("/".into()),
            invitation: Some(offer.token.clone()),
            accept_invitation: true,
            accepted_terms_hash: Some(hash.to_owned()),
            ..AuthOidcStartQuery::default()
        })
        .send_bytes()
        .await;
    assert_eq!(reply.status, 302);
    browser.keep(&reply.headers);
    location(&reply.headers)
}

#[tokio::test]
async fn oidc_requires_a_current_hash_before_start_and_records_it_with_the_verified_join() {
    let sso = sso().await;
    let (root, offer, shown) = invitation(&sso).await;
    for hash in [None, Some("outdated")] {
        let reply = sso
            .world
            .app
            .call(AuthOidcStart, &[&KEY])
            .query(&AuthOidcStartQuery {
                redirect: Some("/".into()),
                invitation: Some(offer.token.clone()),
                accept_invitation: true,
                accepted_terms_hash: hash.map(str::to_owned),
                ..AuthOidcStartQuery::default()
            })
            .send_bytes()
            .await;
        assert_eq!(reply.status, 409);
        let problem = reply.problem();
        assert_eq!(problem.code, ErrorCode::InvitationTermsNotAccepted);
        assert_eq!(problem.extra["terms"], json!(shown));
        let states: i64 = sqlx::query_scalar("SELECT count(*) FROM oidc_states")
            .fetch_one(&sso.world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            states, 0,
            "refused starts make no external round trip state"
        );
        assert!(sso.user_named("dana").await.is_none());
    }
    sso.provider.sign_in_as(dana());
    let mut browser = Browser::default();
    let address = begin(&sso, &mut browser, &offer, &shown.hash).await;
    let state = query_of(&address)["state"].clone();
    let saved: Option<String> =
        sqlx::query_scalar("SELECT accepted_terms_hash FROM oidc_states WHERE state=$1")
            .bind(&state)
            .fetch_one(&sso.world.db.pool)
            .await
            .unwrap();
    assert_eq!(saved.as_deref(), Some(shown.hash.as_str()));
    let back = sso.provider.authorize(&address).await.unwrap();
    assert_eq!(
        sso.come_back(&mut browser, &back).await,
        "/org/invited-team"
    );
    let user = sso.user_named("dana").await.unwrap();
    let accepted: (Option<String>, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT terms_hash,terms_accepted_at FROM org_members WHERE org_id=$1 AND user_id=$2",
    )
    .bind(root)
    .bind(user)
    .fetch_one(&sso.world.db.pool)
    .await
    .unwrap();
    assert_eq!(accepted, (Some(shown.hash.clone()), Some(sso.world.now())));
    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action='member.terms_accepted' AND actor_id=$1",
    )
    .bind(user)
    .fetch_all(&sso.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            json!({"org_id":root,"user_id":user,"user_name":"Dana Whit","terms_hash":shown.hash,"invitation_id":offer.invitation.id})
        ]
    );
    // A later ordinary sign-in is not another invitation acceptance.
    sso.world
        .set_at(
            Target::Node(root),
            "org.terms",
            json!("Changed for new invitees."),
        )
        .await;
    assert_eq!(sso.sign_in(&mut Browser::default(), None).await, "/");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='member.terms_accepted' AND actor_id=$1",
    )
    .bind(user)
    .fetch_one(&sso.world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn terms_changed_during_oidc_return_current_text_and_create_no_account_session_or_join() {
    let sso = sso().await;
    let (root, offer, shown) = invitation(&sso).await;
    sso.provider.sign_in_as(dana());
    let mut browser = Browser::default();
    let address = begin(&sso, &mut browser, &offer, &shown.hash).await;
    let back = sso.provider.authorize(&address).await.unwrap();
    sso.world
        .set_at(
            Target::Node(root),
            "org.terms",
            json!("Changed while the provider was open."),
        )
        .await;
    let current = sso
        .world
        .app
        .call(InvitationLookup, &[&offer.token])
        .send()
        .await
        .ok()
        .terms
        .unwrap();
    assert_ne!(current.hash, shown.hash);
    let sessions = sso.all_sessions().await;
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&sso.world.db.pool)
        .await
        .unwrap();
    let homes: i64 = sqlx::query_scalar("SELECT count(*) FROM orgs")
        .fetch_one(&sso.world.db.pool)
        .await
        .unwrap();
    let query = query_of(&back);
    let reply = sso
        .world
        .app
        .call(AuthOidcCallback, &[&KEY])
        .header("cookie", &browser.header())
        .query(&AuthOidcCallbackQuery {
            code: query.get("code").cloned(),
            state: query.get("state").cloned(),
            ..AuthOidcCallbackQuery::default()
        })
        .send_bytes()
        .await;
    assert_eq!(reply.status, 409);
    let problem = reply.problem();
    assert_eq!(problem.code, ErrorCode::InvitationTermsNotAccepted);
    assert_eq!(problem.extra["terms"], json!(current));
    assert_eq!(sso.all_sessions().await, sessions);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
            .fetch_one(&sso.world.db.pool)
            .await
            .unwrap(),
        users
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM orgs")
            .fetch_one(&sso.world.db.pool)
            .await
            .unwrap(),
        homes
    );
    assert!(sso.user_named("dana").await.is_none());
    assert!(sso.identities_at("sub-dana").await.is_empty());
    let counts: (i32, i64, i64) = sqlx::query_as(
        "SELECT uses, (SELECT count(*) FROM invitation_uses WHERE invitation_id=$1),
           (SELECT count(*) FROM audit_log WHERE action='member.terms_accepted') FROM invitations WHERE id=$1",
    ).bind(offer.invitation.id).fetch_one(&sso.world.db.pool).await.unwrap();
    assert_eq!(counts, (0, 0, 0));
    let states: i64 = sqlx::query_scalar("SELECT count(*) FROM oidc_states")
        .fetch_one(&sso.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        states, 0,
        "the callback retains its existing one-shot browser nonce"
    );
    // Review the replacement text and start once more; the old offer was not consumed.
    let address = begin(&sso, &mut browser, &offer, &current.hash).await;
    let back = sso.provider.authorize(&address).await.unwrap();
    assert_eq!(
        sso.come_back(&mut browser, &back).await,
        "/org/invited-team"
    );
    assert!(sso.user_named("dana").await.is_some());
}
