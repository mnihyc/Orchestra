//! What a change of a page, of the defaults of a place or of the defaults of the system would do, said before it is made, over HTTP: how many pages it reaches, who gains and who loses what, and that nothing is changed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessChoice, HolderInput, NodeDefaultsGet, NodeDefaultsPreview, PageChange, PageChangeAdd,
    PageChangeName, PageChangeRow, PagePreview, PermissionPage, PermissionPageGet,
    PermissionPagePreview, Role,
};
use orch_api_contract::admin::AdminDefaultsPreview;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ownership::{NodeTransfer, TransferBody};
use orch_api_contract::resources::{CreateProvider, ProviderCreate, Upstream};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::secrets::MasterKey;
use orch_types::Id;
use places::{Who, join, make_org, make_project, make_topic, person};
use serde_json::json;

/// Acme with its owner Ada, an admin Dee, a developer Pam who owns the project Web, a viewer Rae, a
/// stranger Sam and a system admin Root; Web holds two topics that follow the defaults and one that
/// was made private.
struct Firm {
    desk: Desk,
    dee: Who,
    pam: Who,
    rae: Who,
    sam: Who,
    root: Who,
    web: Id,
    topics: Vec<Id>,
}

async fn firm() -> Firm {
    install_master_key(MasterKey::new([7; 32]));
    let desk = Desk::new().await;
    let dee = desk.member("dee", "admin").await;
    let pam = desk.member("pam", "developer").await;
    let rae = desk.member("rae", "viewer").await;
    let sam = desk.stranger("sam").await;
    let root = places::admin(&desk.world, "root").await;
    let web = make_project(&desk.world, &desk.ada, desk.org, "Web")
        .await
        .base
        .id;
    desk.world
        .app
        .call(NodeTransfer, &[&web])
        .token(&desk.ada.token)
        .body(&TransferBody {
            user_id: pam.id(),
            keep_previous: Some(false),
        })
        .send()
        .await
        .ok();
    let mut topics = Vec::new();
    for name in ["Plans", "Notes", "Secrets"] {
        topics.push(make_topic(&desk.world, &pam, web, name).await.base.id);
    }
    // The last topic is cut loose from the defaults of its project: nothing there reaches it.
    desk.world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"nodes", &topics[2]],
        )
        .token(&pam.token)
        .body(&PageChange {
            whole: Some(orch_api_contract::access::PageChangeWhole::Private),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    Firm {
        desk,
        dee,
        pam,
        rae,
        sam,
        root,
        web,
        topics,
    }
}

fn naming(user: Id, choice: AccessChoice) -> PageChange {
    PageChange {
        name: Some(vec![PageChangeName {
            holders: vec![HolderInput::User { id: user }],
            choice,
            export: None,
            options: None,
            expires_at: None,
            note: None,
        }]),
        ..PageChange::default()
    }
}

fn adding(permission: &str, holder: HolderInput) -> PageChange {
    PageChange {
        rows: Some(vec![PageChangeRow {
            permission: permission.to_owned(),
            profile: None,
            inherit: None,
            locked: None,
            add: Some(vec![PageChangeAdd {
                holder,
                options: None,
                expires_at: None,
                note: None,
            }]),
            update: None,
            remove: None,
        }]),
        ..PageChange::default()
    }
}

async fn preview_page(
    firm: &Firm,
    by: &Who,
    plural: &str,
    id: Id,
    change: &PageChange,
) -> orch_api_core::testing::Reply<PagePreview> {
    firm.desk
        .world
        .app
        .call(PermissionPagePreview, &[&plural, &id])
        .token(&by.token)
        .body(change)
        .send()
        .await
}

async fn preview_defaults(
    firm: &Firm,
    by: &Who,
    node: Id,
    change: &PageChange,
) -> orch_api_core::testing::Reply<PagePreview> {
    firm.desk
        .world
        .app
        .call(NodeDefaultsPreview, &[&node])
        .token(&by.token)
        .body(change)
        .send()
        .await
}

async fn audited(firm: &Firm) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action LIKE 'permission.%'")
        .fetch_one(&firm.desk.world.db.pool)
        .await
        .unwrap()
}

async fn epoch(firm: &Firm) -> i64 {
    sqlx::query_scalar("SELECT epoch FROM authz_state")
        .fetch_one(&firm.desk.world.db.pool)
        .await
        .unwrap()
}

fn gaining_names(preview: &PagePreview) -> Vec<&str> {
    preview
        .gaining
        .iter()
        .map(|person| person.user.username.as_str())
        .collect()
}

fn losing_names(preview: &PagePreview) -> Vec<&str> {
    preview
        .losing
        .iter()
        .map(|person| person.user.username.as_str())
        .collect()
}

#[tokio::test]
async fn a_preview_of_a_page_says_who_would_gain_and_changes_nothing() {
    let firm = firm().await;
    let before = (
        audited(&firm).await,
        epoch(&firm).await,
        permission_page(&firm, firm.web).await.revision,
    );
    let previewed = preview_page(
        &firm,
        &firm.pam,
        "nodes",
        firm.web,
        &naming(firm.rae.id(), AccessChoice::FullManage),
    )
    .await
    .ok();
    assert_eq!(previewed.pages, 1, "a page is one page");
    assert_eq!(gaining_names(&previewed), ["rae"]);
    let rae = &previewed.gaining[0];
    assert!(
        rae.permissions.contains(&"project.manage".to_owned()),
        "{rae:?}"
    );
    assert!(rae.permissions.contains(&"project.interact".to_owned()));
    assert!(
        !rae.permissions.contains(&"project.view".to_owned()),
        "she views it already: {rae:?}"
    );
    assert_eq!(rae.pages, 1);
    assert!(previewed.losing.is_empty());
    assert_eq!(previewed.more, 0);
    // Nothing was written, moved or audited, and she holds what she held.
    assert_eq!(
        (
            audited(&firm).await,
            epoch(&firm).await,
            permission_page(&firm, firm.web).await.revision
        ),
        before
    );
    let held = firm
        .desk
        .world
        .app
        .call(
            orch_api_contract::access::EffectiveAccessGet,
            &[&"nodes", &firm.web],
        )
        .token(&firm.rae.token)
        .query(&orch_api_contract::access::EffectiveAccessGetQuery {
            user_id: firm.rae.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        !held
            .permissions
            .iter()
            .any(|each| each.permission == "project.manage")
    );
}

async fn permission_page(firm: &Firm, node: Id) -> PermissionPage {
    firm.desk
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &node])
        .token(&firm.pam.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_preview_of_cutting_a_row_loose_says_who_would_lose_it() {
    let firm = firm().await;
    let previewed = preview_page(
        &firm,
        &firm.pam,
        "nodes",
        firm.web,
        &PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "project.view".to_owned(),
                profile: None,
                inherit: Some(false),
                locked: None,
                add: None,
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    let losing = losing_names(&previewed);
    assert!(losing.contains(&"rae"), "{losing:?}");
    assert!(!losing.contains(&"pam"), "its owner keeps it: {losing:?}");
    assert!(previewed.gaining.is_empty());
    let page = permission_page(&firm, firm.web).await;
    let view = page
        .rows
        .iter()
        .find(|row| row.permission == "project.view")
        .unwrap();
    assert!(view.inherit, "the row still follows: nothing was cut");
}

#[tokio::test]
async fn a_preview_is_refused_as_the_change_would_be() {
    let firm = firm().await;
    let by_viewer = preview_page(
        &firm,
        &firm.rae,
        "nodes",
        firm.web,
        &naming(firm.sam.id(), AccessChoice::FullView),
    )
    .await
    .problem();
    assert_eq!(by_viewer.code, ErrorCode::AuthzDenied);
    let stale = preview_page(
        &firm,
        &firm.pam,
        "nodes",
        firm.web,
        &PageChange {
            expected_revision: Some(99),
            ..naming(firm.rae.id(), AccessChoice::FullView)
        },
    )
    .await
    .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let unknown = preview_page(
        &firm,
        &firm.pam,
        "nodes",
        firm.web,
        &adding("project.fly", HolderInput::User { id: firm.rae.id() }),
    )
    .await
    .problem();
    assert_eq!(unknown.code, ErrorCode::RequestInvalid);
    let nowhere = preview_page(&firm, &firm.pam, "nodes", Id::nil(), &PageChange::default())
        .await
        .problem();
    assert!(
        matches!(nowhere.code, ErrorCode::NotFound | ErrorCode::AuthzDenied),
        "{nowhere:?}"
    );
    for plural in ["chats", "gadgets"] {
        let plain = preview_page(&firm, &firm.pam, plural, firm.web, &PageChange::default())
            .await
            .problem();
        assert_eq!(plain.code, ErrorCode::NotFound, "{plural}: {plain:?}");
    }
}

#[tokio::test]
async fn a_preview_of_a_resource_and_of_a_document_says_who_would_gain() {
    let firm = firm().await;
    let provider = firm
        .desk
        .world
        .app
        .call(ProviderCreate, &[])
        .token(&firm.pam.token)
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
        .id;
    let on_provider = preview_page(
        &firm,
        &firm.pam,
        "providers",
        provider,
        &naming(firm.rae.id(), AccessChoice::FullInteract),
    )
    .await
    .ok();
    assert_eq!(gaining_names(&on_provider), ["rae"]);
    assert!(
        on_provider.gaining[0]
            .permissions
            .contains(&"provider.use".to_owned())
    );
    let document = firm.desk.make(&firm.pam, "Plan").await.base.id;
    let on_document = preview_page(
        &firm,
        &firm.pam,
        "documents",
        document,
        &PageChange {
            whole: Some(orch_api_contract::access::PageChangeWhole::Private),
            ..PageChange::default()
        },
    )
    .await
    .ok();
    assert!(
        losing_names(&on_document).contains(&"rae"),
        "making a document private takes it from those who read it through its node: {on_document:?}"
    );
    let refused = preview_page(
        &firm,
        &firm.rae,
        "providers",
        provider,
        &PageChange::default(),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn a_preview_of_a_place_s_defaults_counts_the_pages_that_follow_and_names_who_gains_on_them()
{
    let firm = firm().await;
    let defaults_before = firm
        .desk
        .world
        .app
        .call(NodeDefaultsGet, &[&firm.web])
        .token(&firm.pam.token)
        .send()
        .await
        .ok();
    let before = (audited(&firm).await, epoch(&firm).await);
    let previewed = preview_defaults(
        &firm,
        &firm.pam,
        firm.web,
        &adding(
            "topic.manage",
            HolderInput::Role {
                role: Role::Viewer,
                at: None,
            },
        ),
    )
    .await
    .ok();
    assert_eq!(
        previewed.pages, 2,
        "two topics follow the row and the private one does not: {previewed:?}"
    );
    let rae = previewed
        .gaining
        .iter()
        .find(|person| person.user.username == "rae")
        .expect("a viewer gains it");
    assert_eq!(rae.pages, 2);
    assert!(rae.permissions.contains(&"topic.manage".to_owned()));
    assert!(
        !gaining_names(&previewed).contains(&"pam"),
        "the owner of a topic holds it already: {previewed:?}"
    );
    assert!(previewed.losing.is_empty());
    assert_eq!(
        (audited(&firm).await, epoch(&firm).await),
        before,
        "nothing changed"
    );
    let defaults_after = firm
        .desk
        .world
        .app
        .call(NodeDefaultsGet, &[&firm.web])
        .token(&firm.pam.token)
        .send()
        .await
        .ok();
    assert_eq!(defaults_before, defaults_after);
    // The same change made says what the preview said.
    firm.desk
        .world
        .app
        .call(orch_api_contract::access::NodeDefaultsUpdate, &[&firm.web])
        .token(&firm.pam.token)
        .body(&adding(
            "topic.manage",
            HolderInput::Role {
                role: Role::Viewer,
                at: None,
            },
        ))
        .send()
        .await
        .ok();
    let on_a_topic = firm
        .desk
        .world
        .app
        .call(
            orch_api_contract::access::EffectiveAccessGet,
            &[&"nodes", &firm.topics[0]],
        )
        .token(&firm.rae.token)
        .query(&orch_api_contract::access::EffectiveAccessGetQuery {
            user_id: firm.rae.id(),
        })
        .send()
        .await
        .ok();
    assert!(
        on_a_topic
            .permissions
            .iter()
            .any(|each| each.permission == "topic.manage")
    );
    let on_the_private_one = firm
        .desk
        .world
        .app
        .call(
            orch_api_contract::access::EffectiveAccessGet,
            &[&"nodes", &firm.topics[2]],
        )
        .token(&firm.rae.token)
        .query(&orch_api_contract::access::EffectiveAccessGetQuery {
            user_id: firm.rae.id(),
        })
        .send()
        .await;
    assert!(
        on_the_private_one.body.is_err()
            || !on_the_private_one
                .body
                .unwrap()
                .permissions
                .iter()
                .any(|each| each.permission == "topic.manage")
    );
    // A preview of a thread, which gives nothing to what is made inside it, is refused.
    let thread = preview_defaults(&firm, &firm.pam, firm.sam.id(), &PageChange::default())
        .await
        .problem();
    assert!(
        matches!(thread.code, ErrorCode::NotFound | ErrorCode::AuthzDenied),
        "{thread:?}"
    );
    let refused = preview_defaults(&firm, &firm.rae, firm.web, &PageChange::default())
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn people_are_named_only_as_far_as_the_caller_may_know_them_and_the_rest_are_counted() {
    let firm = firm().await;
    let world = &firm.desk.world;
    // Globex is another root org, with its own people; its Viewers are named on Pam's project.
    let olga = person(world, "olga").await;
    let globex = make_org(world, &olga, "Globex", "globex").await.base.id;
    let gina = person(world, "gina").await;
    join(world, globex, globex, &gina, "viewer", &olga).await;
    // Naming the Viewers of another place takes seeing that place: Olga lets Pam see Globex, by name,
    // which makes her no member of it and nobody she shares an org with.
    world
        .app
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"nodes", &globex],
        )
        .token(&olga.token)
        .body(&naming(firm.pam.id(), AccessChoice::FullView))
        .send()
        .await
        .ok();
    let change = adding(
        "project.interact",
        HolderInput::Role {
            role: Role::Viewer,
            at: Some(globex),
        },
    );
    let by_pam = preview_page(&firm, &firm.pam, "nodes", firm.web, &change)
        .await
        .ok();
    assert!(
        gaining_names(&by_pam).is_empty(),
        "Pam shares no org with them and does not know them: {by_pam:?}"
    );
    assert_eq!(by_pam.more, 2, "Olga and Gina are counted");
    let by_root = preview_page(&firm, &firm.root, "nodes", firm.web, &change)
        .await
        .ok();
    let names = gaining_names(&by_root);
    assert!(
        names.contains(&"olga") && names.contains(&"gina"),
        "{names:?}"
    );
    assert_eq!(by_root.more, 0);
    // A colleague is named to a colleague.
    places::admin_mode(world, &firm.dee, true).await;
    let colleague = preview_page(
        &firm,
        &firm.dee,
        "nodes",
        firm.web,
        &naming(firm.rae.id(), AccessChoice::FullManage),
    )
    .await
    .ok();
    assert_eq!(gaining_names(&colleague), ["rae"]);
}

#[tokio::test]
async fn the_defaults_of_the_system_are_previewed_by_a_system_admin_and_nobody_else() {
    let firm = firm().await;
    let change = adding(
        "project.manage",
        HolderInput::Role {
            role: Role::Viewer,
            at: None,
        },
    );
    let refused = firm
        .desk
        .world
        .app
        .call(AdminDefaultsPreview, &[])
        .token(&firm.dee.token)
        .body(&change)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let before = (audited(&firm).await, epoch(&firm).await);
    let previewed = firm
        .desk
        .world
        .app
        .call(AdminDefaultsPreview, &[])
        .token(&firm.root.token)
        .body(&change)
        .send()
        .await
        .ok();
    assert!(previewed.pages >= 1, "{previewed:?}");
    assert!(
        previewed.orgs.is_some_and(|orgs| orgs >= 1),
        "the preview of the system's defaults says in how many root orgs its pages lie: {previewed:?}"
    );
    assert!(gaining_names(&previewed).contains(&"rae"), "{previewed:?}");
    assert_eq!((audited(&firm).await, epoch(&firm).await), before);
    // Putting them back to what Orch ships changes nothing in a system that has them as shipped.
    let reset = firm
        .desk
        .world
        .app
        .call(AdminDefaultsPreview, &[])
        .token(&firm.root.token)
        .body(&PageChange {
            whole: Some(orch_api_contract::access::PageChangeWhole::Reset),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    assert!(
        reset.gaining.is_empty() && reset.losing.is_empty(),
        "{reset:?}"
    );
    assert_eq!(reset.pages, 0, "{reset:?}");
    assert_eq!(reset.orgs, Some(0), "no page, no org: {reset:?}");
}

#[tokio::test]
async fn nobody_who_is_not_signed_in_previews_anything() {
    let firm = firm().await;
    let app = &firm.desk.world.app;
    let page = app
        .call(PermissionPagePreview, &[&"nodes", &firm.web])
        .body(&PageChange::default())
        .send()
        .await
        .problem();
    assert_eq!(page.code, ErrorCode::AuthRequired);
    let defaults = app
        .call(NodeDefaultsPreview, &[&firm.web])
        .body(&PageChange::default())
        .send()
        .await
        .problem();
    assert_eq!(defaults.code, ErrorCode::AuthRequired);
    let system = app
        .call(AdminDefaultsPreview, &[])
        .body(&PageChange::default())
        .send()
        .await
        .problem();
    assert_eq!(system.code, ErrorCode::AuthRequired);
}
