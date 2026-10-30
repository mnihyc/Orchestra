//! Handing a resource or a document over to another person over HTTP: to whom it may go, what the person who was first keeps, the line it leaves, and that a thing is never left without an owner.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{HistoryList, HistoryListQuery, PermissionPage, PermissionPageGet};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ownership::{ObjectTransfer, TransferBody};
use orch_api_contract::resources::{CreateProvider, ProviderCreate, Resource, Upstream};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::queries::identity::testing::suspend;
use orch_db::secrets::MasterKey;
use orch_types::Id;
use places::Who;
use serde_json::json;

/// Acme with its owner Ada, an admin Dee, a developer Pam, a viewer Rae and a stranger Sam.
struct Firm {
    desk: Desk,
    dee: Who,
    pam: Who,
    rae: Who,
    sam: Who,
}

async fn firm() -> Firm {
    install_master_key(MasterKey::new([7; 32]));
    let desk = Desk::new().await;
    let dee = desk.member("dee", "admin").await;
    let pam = desk.member("pam", "developer").await;
    let rae = desk.member("rae", "viewer").await;
    let sam = desk.stranger("sam").await;
    Firm {
        desk,
        dee,
        pam,
        rae,
        sam,
    }
}

async fn provider_of(firm: &Firm, by: &Who) -> Id {
    firm.desk
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&by.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Team key".to_owned(),
            home_id: firm.desk.org,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
            api_key: Some("sk-test".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider
        .base
        .id
}

async fn pass_on(
    firm: &Firm,
    by: &Who,
    plural: &str,
    id: Id,
    to: Id,
    keep_previous: Option<bool>,
) -> orch_api_core::testing::Reply<Resource> {
    firm.desk
        .world
        .app
        .call(ObjectTransfer, &[&plural, &id])
        .token(&by.token)
        .body(&TransferBody {
            user_id: to,
            keep_previous,
        })
        .send()
        .await
}

async fn page(firm: &Firm, by: &Who, plural: &str, id: Id) -> PermissionPage {
    firm.desk
        .world
        .app
        .call(PermissionPageGet, &[&plural, &id])
        .token(&by.token)
        .send()
        .await
        .ok()
}

fn names(page: &PermissionPage) -> Vec<String> {
    page.owners
        .iter()
        .filter_map(|entry| match &entry.holder {
            orch_api_contract::access::Holder::User { user } => Some(user.username.clone()),
            _ => None,
        })
        .collect()
}

fn owner_of(resource: &Resource) -> String {
    match resource {
        Resource::Provider(provider) => provider.owner.username.clone(),
        Resource::Document(_) => String::new(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_first_owner_passes_a_provider_on_and_stays_an_owner_by_default() {
    let firm = firm().await;
    let id = provider_of(&firm, &firm.pam).await;
    let handed = pass_on(&firm, &firm.pam, "providers", id, firm.rae.id(), None)
        .await
        .ok();
    assert_eq!(owner_of(&handed), "rae");
    let now = page(&firm, &firm.rae, "providers", id).await;
    assert_eq!(now.owner.as_ref().unwrap().username, "rae");
    let owners = names(&now);
    assert_eq!(
        owners.first().map(String::as_str),
        Some("rae"),
        "first among the owners: {owners:?}"
    );
    assert!(
        owners.contains(&"pam".to_owned()),
        "the person who was first stays: {owners:?}"
    );
    // The line it leaves is in the history of the provider, for whoever may view it.
    let lines = firm
        .desk
        .world
        .app
        .call(HistoryList, &[&"providers", &id])
        .token(&firm.pam.token)
        .query(&HistoryListQuery::default())
        .send()
        .await
        .ok();
    let line = lines
        .items
        .iter()
        .find(|line| line.action == "resource.transferred")
        .expect("the passing on is in its history");
    assert!(line.text.contains("rae"), "{}", line.text);
}

#[tokio::test]
async fn the_person_who_was_first_can_be_taken_out_in_the_same_step_and_the_row_never_empties() {
    let firm = firm().await;
    let id = provider_of(&firm, &firm.pam).await;
    pass_on(
        &firm,
        &firm.pam,
        "providers",
        id,
        firm.rae.id(),
        Some(false),
    )
    .await
    .ok();
    let now = page(&firm, &firm.rae, "providers", id).await;
    assert_eq!(names(&now), ["rae"]);
    // The old owner owns nothing of it now.
    let refused = pass_on(&firm, &firm.pam, "providers", id, firm.pam.id(), None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // Handing it to the person who is first already changes nothing.
    let audited = |world: &support::World| {
        let pool = world.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM audit_log WHERE action = 'resource.transferred'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let before = audited(&firm.desk.world).await;
    let same = pass_on(
        &firm,
        &firm.rae,
        "providers",
        id,
        firm.rae.id(),
        Some(false),
    )
    .await
    .ok();
    assert_eq!(owner_of(&same), "rae");
    assert_eq!(audited(&firm.desk.world).await, before);
    assert_eq!(
        names(&page(&firm, &firm.rae, "providers", id).await),
        ["rae"]
    );
}

#[tokio::test]
async fn an_admin_of_its_home_passes_it_on_and_somebody_who_owns_nothing_is_refused() {
    let firm = firm().await;
    let id = provider_of(&firm, &firm.pam).await;
    for refused in [&firm.rae, &firm.sam] {
        let problem = pass_on(&firm, refused, "providers", id, refused.id(), None)
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::AuthzDenied, "{problem:?}");
    }
    // An admin of its home passes it on as its admin, with the session's As admin switch on.
    places::admin_mode(&firm.desk.world, &firm.dee, true).await;
    let by_admin = pass_on(
        &firm,
        &firm.dee,
        "providers",
        id,
        firm.rae.id(),
        Some(false),
    )
    .await
    .ok();
    assert_eq!(owner_of(&by_admin), "rae");
}

#[tokio::test]
async fn it_goes_only_to_an_active_member_of_the_org_the_thing_lies_in() {
    let firm = firm().await;
    let id = provider_of(&firm, &firm.pam).await;
    for (to, why) in [
        (firm.sam.id(), "a stranger to the org"),
        (Id::nil(), "nobody"),
    ] {
        let problem = pass_on(&firm, &firm.pam, "providers", id, to, None)
            .await
            .problem();
        assert_eq!(
            problem.code,
            ErrorCode::RequestInvalid,
            "{why}: {problem:?}"
        );
    }
    {
        let mut conn = firm.desk.world.conn().await;
        suspend(&mut conn, firm.rae.id()).await.unwrap();
    }
    let suspended = pass_on(&firm, &firm.pam, "providers", id, firm.rae.id(), None)
        .await
        .problem();
    assert_eq!(suspended.code, ErrorCode::RequestInvalid, "{suspended:?}");
    assert_eq!(
        page(&firm, &firm.pam, "providers", id)
            .await
            .owner
            .unwrap()
            .username,
        "pam",
        "nothing was changed"
    );
}

#[tokio::test]
async fn a_document_is_passed_on_like_a_provider_and_leaves_its_line() {
    let firm = firm().await;
    let document = firm.desk.make(&firm.pam, "Plan").await.base.id;
    let handed = pass_on(
        &firm,
        &firm.pam,
        "documents",
        document,
        firm.rae.id(),
        Some(false),
    )
    .await
    .ok();
    assert!(matches!(handed, Resource::Document(_)), "{handed:?}");
    let now = page(&firm, &firm.rae, "documents", document).await;
    assert_eq!(now.owner.as_ref().unwrap().username, "rae");
    assert_eq!(names(&now), ["rae"]);
    let lines = firm
        .desk
        .world
        .app
        .call(HistoryList, &[&"documents", &document])
        .token(&firm.rae.token)
        .query(&HistoryListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        lines
            .items
            .iter()
            .any(|line| line.action == "document.transferred"),
        "{:?}",
        lines
            .items
            .iter()
            .map(|line| &line.action)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_chat_and_a_kind_nothing_serves_are_answered_plainly() {
    let firm = firm().await;
    let id = provider_of(&firm, &firm.pam).await;
    for plural in ["chats", "gadgets", "mcp-servers", "mounts"] {
        let problem = pass_on(&firm, &firm.pam, plural, id, firm.rae.id(), None)
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::NotFound, "{plural}: {problem:?}");
    }
    let missing = pass_on(
        &firm,
        &firm.pam,
        "providers",
        orch_types::new_id(),
        firm.rae.id(),
        None,
    )
    .await
    .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn nobody_who_is_not_signed_in_hands_anything_over() {
    let firm = firm().await;
    let id = provider_of(&firm, &firm.pam).await;
    let problem = firm
        .desk
        .world
        .app
        .call(ObjectTransfer, &[&"providers", &id])
        .body(&TransferBody {
            user_id: firm.rae.id(),
            keep_previous: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthRequired);
}
