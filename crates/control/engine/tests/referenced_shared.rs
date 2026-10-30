//! Referenced-by search lists only what the agent may read now, with the lines around the mentions, and leaves no trace of the rest.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_engine::tools::definitions::Code;
use orch_link_contract::commands::OrchTool;
use serde_json::json;
use shared::*;

#[tokio::test]
async fn referenced_by_lists_the_documents_the_agent_may_read_with_the_line_around_each_mention() {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Writing").await;
    let target = world
        .document_beside(topic, "Referenced plan", &["The release plan"])
        .await;
    let open = world
        .document_beside(topic, "Readable notes", &["Placeholder"])
        .await;
    let secret = world
        .document_beside(topic, "Secret layoffs", &["Placeholder"])
        .await;
    for (document, before) in [(open, "Decided: see "), (secret, "Hidden: see ")] {
        sqlx::query("UPDATE document_blocks SET content=$2 WHERE document_id=$1")
            .bind(document)
            .bind(json!([
                {"type":"text","text":before},
                {"type":"ref","kind":"document","id":target,"label":"Old title"},
                {"type":"text","text":" for the dates."}
            ]))
            .execute(world.pool.pool())
            .await
            .unwrap();
    }
    share(
        &world, "document", target, "live", "doc.view", "any", reader,
    )
    .await;
    let grant = share(&world, "document", open, "live", "doc.view", "any", reader).await;
    let found = ok(call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"referenced_by":format!("id:{target}")}),
    )
    .await);
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["id"], json!(open));
    assert_eq!(hits[0]["mentions"], 1);
    assert!(
        hits[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("Readable notes.md")
    );
    let text = hits[0]["lines"][0]["text"].as_str().unwrap();
    assert_eq!(
        text, "Decided: see [Referenced plan] for the dates.",
        "the target under its name now"
    );
    assert_eq!(
        found["searched"]["entries"], 1,
        "the unshared source is not counted"
    );
    let everything = found.to_string();
    assert!(!everything.contains("Secret layoffs") && !everything.contains("Hidden: see"));
    assert!(!everything.contains(&secret.to_string()));
    // Giving the agent the second document brings it, and only it, into the list.
    share(
        &world, "document", secret, "live", "doc.view", "any", reader,
    )
    .await;
    let both = ok(call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"referenced_by":format!("id:{target}")}),
    )
    .await);
    assert_eq!(both["hits"].as_array().unwrap().len(), 2);
    sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
        .bind(grant)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    let revoked = ok(call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"referenced_by":format!("id:{target}")}),
    )
    .await);
    let ids: Vec<_> = revoked["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["id"].clone())
        .collect();
    assert_eq!(
        ids,
        vec![json!(secret)],
        "revocation applies on the next call"
    );
}

#[tokio::test]
async fn referenced_by_refuses_a_query_that_would_be_ignored_and_a_document_it_may_not_read() {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Writing").await;
    let unshared = world
        .document_beside(topic, "Unshared notes", &["Not given to this agent"])
        .await;
    for arguments in [
        json!({"referenced_by":"docs/Plan.md","query":"release"}),
        json!({"referenced_by":"docs/Plan.md","related_to":"docs/Plan.md"}),
        json!({"referenced_by":format!("id:{reader}")}),
    ] {
        let refusal = call(&world, reader, OrchTool::SearchShared, arguments).await;
        assert_eq!(refused(&refusal).0, Code::Invalid.as_str());
    }
    let hidden = call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"referenced_by":format!("id:{unshared}")}),
    )
    .await;
    assert_eq!(refused(&hidden).0, "not_found");
}

#[tokio::test]
async fn referenced_by_pages_past_unshared_sources_and_binds_opaque_cursors_to_the_list() {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let other = world.thread("Other reader").await;
    let topic = world.topic("Writing").await;
    let target = world
        .document_beside(topic, "Plan", &["The release plan"])
        .await;
    for thread in [reader, other] {
        share(
            &world, "document", target, "live", "doc.view", "any", thread,
        )
        .await;
    }
    world
        .set(world.topic, "documents.links.access_checks", json!(2))
        .await;
    let mut visible = Vec::new();
    let mut hidden = Vec::new();
    for number in 0..6 {
        for readable in [true, false] {
            let name = if readable {
                format!("Open {number}")
            } else {
                format!("Hidden {number}")
            };
            let source = world.document_beside(topic, &name, &["Placeholder"]).await;
            sqlx::query("UPDATE document_blocks SET content=$2 WHERE document_id=$1")
                .bind(source)
                .bind(json!([{"type":"ref","kind":"document","id":target,"label":"Plan"}]))
                .execute(world.pool.pool())
                .await
                .unwrap();
            if readable {
                share(
                    &world, "document", source, "live", "doc.view", "any", reader,
                )
                .await;
                visible.push(json!(source));
            } else {
                hidden.push(source);
            }
        }
    }
    // A full initial scan window may contain no readable source at all.
    for number in 0..3 {
        let source = world
            .document_beside(topic, &format!("Hidden newest {number}"), &["Placeholder"])
            .await;
        sqlx::query("UPDATE document_blocks SET content=$2 WHERE document_id=$1")
            .bind(source)
            .bind(json!([{"type":"ref","kind":"document","id":target,"label":"Plan"}]))
            .execute(world.pool.pool())
            .await
            .unwrap();
        hidden.push(source);
    }
    let mut cursor = None;
    let mut first_cursor = None;
    let mut saw_empty_continuation = false;
    let mut seen = Vec::new();
    for _ in 0..20 {
        let page = ok(call(
            &world,
            reader,
            OrchTool::SearchShared,
            json!({"referenced_by":format!("id:{target}"),"max_results":1,"cursor":cursor}),
        )
        .await);
        let hits = page["hits"].as_array().unwrap();
        assert!(hits.len() <= 1);
        seen.extend(hits.iter().map(|hit| hit["id"].clone()));
        assert!(!page.to_string().contains("Hidden"));
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if let Some(token) = &cursor {
            saw_empty_continuation |= hits.is_empty();
            first_cursor.get_or_insert_with(|| token.clone());
            let decoded = URL_SAFE_NO_PAD.decode(token).unwrap();
            let exposed = String::from_utf8_lossy(&decoded);
            assert!(hidden.iter().all(|id| !exposed.contains(&id.to_string())));
        } else {
            break;
        }
    }
    assert!(cursor.is_none(), "the keyset pages end");
    assert!(
        saw_empty_continuation,
        "an empty access-budget page still advances"
    );
    assert_eq!(
        seen.len(),
        visible.len(),
        "no repeated or skipped readable source"
    );
    seen.sort_by_key(serde_json::Value::to_string);
    visible.sort_by_key(serde_json::Value::to_string);
    assert_eq!(seen, visible);
    let token = first_cursor.unwrap();
    for (thread, names) in [(other, None), (reader, Some("Open*"))] {
        let answer = call(
            &world,
            thread,
            OrchTool::SearchShared,
            json!({"referenced_by":format!("id:{target}"),"cursor":token,"names":names}),
        )
        .await;
        let (code, message) = refused(&answer);
        assert_eq!(
            code,
            Code::Invalid.as_str(),
            "a cursor is bound to its thread and filters"
        );
        assert!(message.contains("another referenced_by list"), "{message}");
    }
}
