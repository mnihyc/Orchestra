//! The destinations of a node over HTTP: where what happens at an org, a project or a topic is sent for nobody in particular, set up by whoever manages the node, checked like a channel, kept with sealed secrets that are never returned, changed, removed and tried.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod outside;
mod places;
mod support;

use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::notifications::{
    Destination, DestinationDelete, DestinationLevel, DestinationTest, DestinationUpdate,
    DestinationUpdateBody, NodeDestinationCreate, NodeDestinationCreateBody, NodeDestinationList,
    NotificationTestReply,
};
use orch_api_core::testing::Reply;
use orch_types::Id;
use outside::{FarSide, Outside};
use places::{Who, make_project, make_topic};
use serde_json::json;
use std::collections::BTreeMap;

struct Place {
    o: Outside,
    project: Id,
    topic: Id,
}

async fn place() -> Place {
    let o = Outside::new().await;
    let project = make_project(&o.world, &o.bea, o.acme, "Web").await.base.id;
    let topic = make_topic(&o.world, &o.bea, project, "Login").await.base.id;
    // What an org or a person sets up reaches this machine only where the system lets it.
    o.world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;
    Place { o, project, topic }
}

fn room(url: &str) -> NodeDestinationCreateBody {
    NodeDestinationCreateBody {
        channel: None,
        name: "Ops room".to_owned(),
        config: json!({ "url": url }),
        secrets: Some(BTreeMap::from([(
            "secret".to_owned(),
            "room-signing-secret".to_owned(),
        )])),
        templates: None,
        kinds: None,
        level: None,
        enabled: None,
    }
}

async fn add(
    p: &Place,
    by: &Who,
    node: Id,
    body: &NodeDestinationCreateBody,
) -> Reply<Destination> {
    p.o.world
        .app
        .call(NodeDestinationCreate, &[&node])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

async fn list(p: &Place, by: &Who, node: Id) -> Reply<Vec<Destination>> {
    p.o.world
        .app
        .call(NodeDestinationList, &[&node])
        .token(&by.token)
        .send()
        .await
}

async fn patch(p: &Place, by: &Who, id: Id, body: &DestinationUpdateBody) -> Reply<Destination> {
    p.o.world
        .app
        .call(DestinationUpdate, &[&id])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

async fn remove(p: &Place, by: &Who, id: Id) -> Reply<()> {
    p.o.world
        .app
        .call(DestinationDelete, &[&id])
        .token(&by.token)
        .send()
        .await
}

async fn try_it(p: &Place, by: &Who, id: Id) -> Reply<NotificationTestReply> {
    p.o.world
        .app
        .call(DestinationTest, &[&id])
        .token(&by.token)
        .send()
        .await
}

fn paths(problem: &Problem) -> Vec<String> {
    problem
        .extra
        .get("errors")
        .and_then(|errors| errors.as_array())
        .into_iter()
        .flatten()
        .map(|fault| fault["path"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_destination_is_set_up_at_an_org_a_project_or_a_topic_by_whoever_holds_settings_manage_there()
 {
    let p = place().await;
    let far = FarSide::taking().await;
    for node in [p.o.acme, p.o.research, p.project, p.topic] {
        let made = add(&p, &p.o.bea, node, &room(&far.url)).await.ok();
        assert_eq!(made.node.id, node);
        assert_eq!(
            made.channel, "webhook",
            "a webhook unless another notifier is given"
        );
        assert_eq!(
            made.level,
            DestinationLevel::Important,
            "important unless given"
        );
        assert!(made.enabled && made.kinds.is_empty());
        assert_eq!(made.config["url"], far.url.as_str());
        assert_eq!(made.secrets_set, ["secret"]);
    }
    // A developer of the org holds the right to work in it and not to set its policies; a viewer and a stranger do not either.
    for who in [&p.o.cai, &p.o.dee, &p.o.fay] {
        for node in [p.o.acme, p.project, p.topic] {
            let refused = add(&p, who, node, &room(&far.url)).await.problem();
            assert_eq!(refused.code, ErrorCode::AuthzDenied);
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM destinations")
        .fetch_one(&p.o.world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 4);
}

#[tokio::test]
async fn the_destinations_of_a_node_are_listed_to_whoever_manages_it() {
    let p = place().await;
    let far = FarSide::taking().await;
    add(&p, &p.o.bea, p.project, &room(&far.url)).await.ok();
    let mut second = room(&far.url);
    second.name = "Build system".to_owned();
    second.level = Some(DestinationLevel::All);
    add(&p, &p.o.bea, p.project, &second).await.ok();
    add(&p, &p.o.bea, p.topic, &room(&far.url)).await.ok();

    let found = list(&p, &p.o.bea, p.project).await.ok();
    assert_eq!(
        found
            .iter()
            .map(|each| each.name.as_str())
            .collect::<Vec<_>>(),
        ["Ops room", "Build system"],
        "in the order they were set up, and only those of this node"
    );
    assert_eq!(found[1].level, DestinationLevel::All);
    for who in [&p.o.cai, &p.o.dee, &p.o.fay] {
        assert_eq!(
            list(&p, who, p.project).await.problem().code,
            ErrorCode::AuthzDenied
        );
    }
}

#[tokio::test]
async fn a_destination_is_checked_against_its_notifier_and_the_list_of_private_hosts() {
    let p = place().await;
    let far = FarSide::taking().await;
    // Nothing needed, nothing known, nothing that is only a channel.
    let mut no_url = room(&far.url);
    no_url.config = json!({});
    assert_eq!(
        paths(&add(&p, &p.o.bea, p.project, &no_url).await.problem()),
        ["config.url"]
    );
    let mut unknown_field = room(&far.url);
    unknown_field.config = json!({ "url": far.url, "colour": "red" });
    assert_eq!(
        paths(&add(&p, &p.o.bea, p.project, &unknown_field).await.problem()),
        ["config.colour"]
    );
    for channel in ["carrier-pigeon", "email", "telegram", "push"] {
        let mut body = room(&far.url);
        body.channel = Some(channel.to_owned());
        assert_eq!(
            paths(&add(&p, &p.o.bea, p.project, &body).await.problem()),
            ["channel"],
            "{channel}"
        );
    }
    let mut nameless = room(&far.url);
    nameless.name = String::new();
    assert_eq!(
        paths(&add(&p, &p.o.bea, p.project, &nameless).await.problem()),
        ["name"]
    );
    // A destination is held to the list of private hosts; a host on it is taken.
    p.o.world
        .set("outbound.allowed_private_hosts", json!([]))
        .await;
    let refused = add(&p, &p.o.bea, p.project, &room(&far.url))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    // The fault places the address; its explanation names a setting, which a public problem keeps
    // to the catalogue's sentence.
    assert_eq!(paths(&refused), ["config"], "{refused:?}");
    assert_eq!(
        refused.extra["errors"][0]["message"],
        orch_api_contract::messages::for_code(ErrorCode::RequestInvalid)
    );
    p.o.world
        .set("outbound.allowed_private_hosts", json!(["127.0.0.1"]))
        .await;
    add(&p, &p.o.bea, p.project, &room(&far.url)).await.ok();
}

#[tokio::test]
async fn only_the_kinds_a_destination_is_ever_sent_can_be_named() {
    let p = place().await;
    let far = FarSide::taking().await;
    for kind in ["mention", "chat.message", "doc.comment", "turn.exploded"] {
        let mut body = room(&far.url);
        body.kinds = Some(vec![kind.to_owned()]);
        assert_eq!(
            paths(&add(&p, &p.o.bea, p.project, &body).await.problem()),
            ["kinds"],
            "{kind}"
        );
    }
    let mut body = room(&far.url);
    body.kinds = Some(vec![
        "turn.failed".to_owned(),
        "thread.blocked".to_owned(),
        "turn.failed".to_owned(),
    ]);
    let made = add(&p, &p.o.bea, p.project, &body).await.ok();
    assert_eq!(made.kinds, ["turn.failed", "thread.blocked"]);
}

#[tokio::test]
async fn the_secret_of_a_destination_is_sealed_and_cannot_be_read_back() {
    let p = place().await;
    let far = FarSide::taking().await;
    let made = add(&p, &p.o.bea, p.project, &room(&far.url)).await.ok();
    let mut said = serde_json::to_string(&made).unwrap();
    said.push_str(&serde_json::to_string(&list(&p, &p.o.bea, p.project).await.ok()).unwrap());
    assert!(!said.contains("room-signing-secret"), "{said}");
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT name, sealed FROM secrets WHERE owner_kind = 'destination' AND owner_id = $1",
    )
    .bind(made.id)
    .fetch_all(&p.o.world.db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "secret");
    assert!(
        !rows[0]
            .1
            .windows("room-signing".len())
            .any(|window| window == b"room-signing")
    );
    let trail: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT action, details FROM audit_log WHERE object_id = $1")
            .bind(made.id)
            .fetch_all(&p.o.world.db.pool)
            .await
            .unwrap();
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].0, "destination.created");
    assert!(!trail[0].1.to_string().contains("room-signing-secret"));
}

#[tokio::test]
async fn a_destination_is_changed_and_removed_by_whoever_manages_its_node_and_by_nobody_else() {
    let p = place().await;
    let far = FarSide::taking().await;
    let made = add(&p, &p.o.bea, p.project, &room(&far.url)).await.ok();
    let change = |body: DestinationUpdateBody| {
        let p = &p;
        let id = made.id;
        async move { patch(p, &p.o.bea, id, &body).await }
    };
    let renamed = change(DestinationUpdateBody {
        name: Some("Ops".to_owned()),
        level: Some(DestinationLevel::All),
        kinds: Some(vec!["turn.failed".to_owned()]),
        enabled: Some(false),
        ..DestinationUpdateBody::default()
    })
    .await
    .ok();
    assert_eq!(renamed.name, "Ops");
    assert_eq!(renamed.level, DestinationLevel::All);
    assert_eq!(renamed.kinds, ["turn.failed"]);
    assert!(!renamed.enabled);
    assert_eq!(
        renamed.config["url"],
        far.url.as_str(),
        "what is left out stays"
    );
    assert_eq!(renamed.secrets_set, ["secret"]);

    // Its fields are looked at again, and it is switched on.
    let again = change(DestinationUpdateBody {
        config: Some(json!({ "url": far.url })),
        secrets: Some(BTreeMap::from([(
            "secret".to_owned(),
            "another".to_owned(),
        )])),
        enabled: Some(true),
        ..DestinationUpdateBody::default()
    })
    .await
    .ok();
    assert!(again.enabled);
    let bad = change(DestinationUpdateBody {
        config: Some(json!({})),
        ..DestinationUpdateBody::default()
    })
    .await
    .problem();
    assert_eq!(paths(&bad), ["config.url"]);
    let wrong_kinds = change(DestinationUpdateBody {
        kinds: Some(vec!["mention".to_owned()]),
        ..DestinationUpdateBody::default()
    })
    .await
    .problem();
    assert_eq!(paths(&wrong_kinds), ["kinds"]);

    for who in [&p.o.cai, &p.o.dee, &p.o.fay] {
        assert_eq!(
            patch(&p, who, made.id, &DestinationUpdateBody::default())
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(
            remove(&p, who, made.id).await.problem().code,
            ErrorCode::AuthzDenied
        );
        assert_eq!(
            try_it(&p, who, made.id).await.problem().code,
            ErrorCode::AuthzDenied
        );
    }
    assert_eq!(remove(&p, &p.o.bea, made.id).await.status, 204);
    assert_eq!(
        p.o.count("SELECT count(*) FROM secrets WHERE owner_id = $1", made.id)
            .await,
        0,
        "its secrets go with it"
    );
    assert!(list(&p, &p.o.bea, p.project).await.ok().is_empty());
    assert_eq!(
        remove(&p, &p.o.bea, made.id).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        patch(
            &p,
            &p.o.bea,
            orch_types::new_id(),
            &DestinationUpdateBody::default()
        )
        .await
        .problem()
        .code,
        ErrorCode::NotFound
    );
    let trail: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_log WHERE object_id = $1 ORDER BY at, id")
            .bind(made.id)
            .fetch_all(&p.o.world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        trail,
        [
            "destination.created",
            "destination.updated",
            "destination.updated",
            "destination.deleted"
        ]
    );
}

#[tokio::test]
async fn a_test_delivers_a_sample_to_the_destination_and_returns_what_the_far_side_answered() {
    let p = place().await;
    let broken = FarSide::answering(502, "bad gateway from the room").await;
    let made = add(&p, &p.o.bea, p.topic, &room(&broken.url)).await.ok();
    let tried = try_it(&p, &p.o.bea, made.id).await.ok();
    assert!(!tried.ok);
    assert!(tried.error.as_deref().unwrap().contains("502"), "{tried:?}");
    let hit = broken.hits.lock().unwrap()[0].clone();
    assert!(
        hit.signature.as_deref().unwrap().starts_with("sha256="),
        "signed with the secret"
    );
    let posted: serde_json::Value = serde_json::from_str(&hit.body).unwrap();
    assert_eq!(posted["kind"], "milestone");
    assert_eq!(
        posted["recipient"],
        serde_json::Value::Null,
        "a node's destination has nobody in particular"
    );
    // The reason is shown on the destination.
    let shown = list(&p, &p.o.bea, p.topic).await.ok();
    assert!(shown[0].last_error.as_deref().unwrap().contains("502"));

    // Once it is mended the reason goes.
    let mended = FarSide::taking().await;
    patch(
        &p,
        &p.o.bea,
        made.id,
        &DestinationUpdateBody {
            config: Some(json!({ "url": mended.url })),
            ..DestinationUpdateBody::default()
        },
    )
    .await
    .ok();
    assert!(try_it(&p, &p.o.bea, made.id).await.ok().ok);
    let shown = list(&p, &p.o.bea, p.topic).await.ok();
    assert_eq!(shown[0].last_error, None);
    assert!(shown[0].last_ok_at.is_some());
}

#[tokio::test]
async fn nobody_who_is_not_signed_in_sets_up_or_changes_a_destination() {
    let p = place().await;
    let id = orch_types::new_id();
    let problems = [
        p.o.world
            .app
            .call(NodeDestinationList, &[&p.project])
            .send()
            .await
            .problem(),
        p.o.world
            .app
            .call(NodeDestinationCreate, &[&p.project])
            .body(&room("https://room.example/hook"))
            .send()
            .await
            .problem(),
        p.o.world
            .app
            .call(DestinationUpdate, &[&id])
            .body(&DestinationUpdateBody::default())
            .send()
            .await
            .problem(),
        p.o.world
            .app
            .call(DestinationDelete, &[&id])
            .send()
            .await
            .problem(),
        p.o.world
            .app
            .call(DestinationTest, &[&id])
            .send()
            .await
            .problem(),
    ];
    for problem in problems {
        assert_eq!(problem.code, ErrorCode::AuthRequired);
    }
}
