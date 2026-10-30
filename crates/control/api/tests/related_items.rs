//! Related lists share weighted ranking, current reading access and generation-bound whole-item vectors.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::documents::{NewDocumentAccess, NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::search::{
    DocumentRelatedList, RelatedBy, RelatedItems, RelatedMeaning, RelatedPlace, RelatedReference,
};
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_types::{Id, Json, new_id};
use rig::Scene;
use serde_json::json;

async fn document(s: &Scene, node: Id, title: &str, text: &str, private: bool) -> Id {
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&node])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.into(),
            blocks: None,
            markdown: Some(text.into()),
            access: private.then_some(NewDocumentAccess::Private),
        })
        .send()
        .await
        .ok()
        .base
        .id
}

async fn set(s: &Scene, node: Id, key: &str, value: Json) {
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&node, &key])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value,
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

async fn related(s: &Scene, source: Id, token: &str) -> RelatedItems {
    s.rig
        .world
        .app
        .call(DocumentRelatedList, &[&source])
        .token(token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_documents_related_tab_loads_for_its_owner_and_viewer_with_current_privacy() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let source = document(&s, topic, "Related tab source", "Source", false).await;
    let visible = document(&s, topic, "Visible related peer", "Peer", false).await;
    let private = document(&s, topic, "Private related peer", "Private", true).await;
    set(
        &s,
        topic,
        "related.weights",
        json!({"meaning":"0","place":"1","references":"0"}),
    )
    .await;
    for (token, owns_private) in [(&s.ada.token, true), (&s.cai.token, false)] {
        // The Links tab initially sends this ordinary GET without a cursor or special inspection mode.
        let response = s
            .rig
            .world
            .app
            .call(DocumentRelatedList, &[&source])
            .token(token)
            .send()
            .await;
        assert_eq!(
            response.status, 200,
            "the Related tab must load for a reader"
        );
        let answer = response.ok();
        assert!(answer.items.iter().any(|item| item.id == visible));
        assert!(!answer.items.iter().any(|item| item.id == source));
        assert_eq!(
            answer.items.iter().any(|item| item.id == private),
            owns_private,
            "the query repair preserves current document privacy"
        );
    }
    let outsider = s
        .rig
        .world
        .app
        .call(DocumentRelatedList, &[&source])
        .token(&s.bea.token)
        .send()
        .await;
    assert_eq!(outsider.status, 404);
    assert_eq!(s.rig.commands.issued_of("provider.embed"), 0);
}

#[tokio::test]
async fn place_and_reference_weights_come_from_the_documents_settings_and_hidden_items_never_escape()
 {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let project = s.tree.project.base.id;
    let org = s.tree.org.base.id;
    let far = document(&s, org, "Referenced", "Release notes", false).await;
    let source = document(
        &s,
        topic,
        "Plan",
        &format!("[Release notes](/documents/{far})"),
        false,
    )
    .await;
    let near = document(&s, topic, "Same topic", "Work beside the plan", false).await;
    let project_doc = document(&s, project, "Same project", "Project work", false).await;
    let hidden = document(
        &s,
        topic,
        "Private draft",
        &format!("[Plan](/documents/{source})"),
        true,
    )
    .await;
    set(
        &s,
        org,
        "related.weights",
        json!({"meaning":"0","place":"1","references":"0"}),
    )
    .await;
    let by_place = related(&s, source, &s.cai.token).await;
    assert_eq!(by_place.meaning, RelatedMeaning::Off);
    let position = |id| {
        by_place
            .items
            .iter()
            .position(|item| item.id == id)
            .unwrap()
    };
    assert!(position(near) < position(project_doc));
    assert!(position(project_doc) < position(far));
    assert!(
        !by_place
            .items
            .iter()
            .any(|item| item.id == source || item.id == hidden)
    );
    assert_eq!(
        by_place
            .items
            .iter()
            .find(|item| item.id == near)
            .unwrap()
            .place,
        Some(RelatedPlace::Topic)
    );
    assert!(
        by_place
            .items
            .iter()
            .all(|item| item.related_by == [RelatedBy::Place])
    );

    set(
        &s,
        topic,
        "related.weights",
        json!({"meaning":"0","place":"0","references":"1"}),
    )
    .await;
    let by_reference = related(&s, source, &s.cai.token).await;
    assert_eq!(by_reference.items.len(), 1);
    assert_eq!(by_reference.items[0].id, far);
    assert_eq!(
        by_reference.items[0].references,
        Some(RelatedReference::Outgoing)
    );
    assert_eq!(by_reference.items[0].related_by, [RelatedBy::References]);
    let owner = related(&s, source, &s.ada.token).await;
    assert!(
        owner
            .items
            .iter()
            .any(|item| item.id == hidden && item.references == Some(RelatedReference::Incoming))
    );

    assert_eq!(
        s.rig
            .world
            .app
            .call(DocumentRelatedList, &[&hidden])
            .token(&s.cai.token)
            .send()
            .await
            .status,
        404
    );
    assert_eq!(
        s.rig
            .world
            .app
            .call(DocumentRelatedList, &[&source])
            .token(&s.bea.token)
            .send()
            .await
            .status,
        404
    );
    set(&s, topic, "related.count", json!(0)).await;
    assert!(related(&s, source, &s.ada.token).await.items.is_empty());
}

#[tokio::test]
async fn duplicate_links_shared_targets_and_removed_references_have_their_current_meaning() {
    let s = Scene::new().await;
    let node = s.tree.topic.base.id;
    set(
        &s,
        node,
        "related.weights",
        json!({"meaning":"0","place":"0","references":"1"}),
    )
    .await;
    let target = document(&s, node, "Target", "A referenced object", false).await;
    let source = document(
        &s,
        node,
        "Source",
        &format!("[one](/documents/{target}) [again](/documents/{target})"),
        false,
    )
    .await;
    let shared = document(
        &s,
        node,
        "Shared target",
        &format!("[same target](/documents/{target})"),
        false,
    )
    .await;
    let first = related(&s, source, &s.cai.token).await;
    assert!(
        first
            .items
            .iter()
            .any(|item| item.id == target && item.references == Some(RelatedReference::Outgoing))
    );
    assert!(
        first
            .items
            .iter()
            .any(|item| item.id == shared && item.references == Some(RelatedReference::Shared))
    );
    assert_eq!(
        first.items.iter().filter(|item| item.id == target).count(),
        1
    );
    sqlx::query("UPDATE document_blocks SET content='[{\"type\":\"text\",\"text\":\"No references now\"}]' WHERE document_id=$1")
        .bind(source).execute(s.rig.db.pool()).await.unwrap();
    assert!(
        related(&s, source, &s.cai.token).await.items.is_empty(),
        "editing the block removes its indexed edges"
    );
}

#[tokio::test]
async fn the_same_document_folder_outranks_a_sibling_folder_and_supplies_only_permitted_name_context()
 {
    let s = Scene::new().await;
    let node = s.tree.topic.base.id;
    let source = document(&s, node, "Source", "A plan", false).await;
    let beside = document(&s, node, "Notes", "Inside the same folder", false).await;
    let sibling = document(&s, node, "Other notes", "Inside another folder", false).await;
    let folder = new_id();
    let other = new_id();
    for (id, name) in [(folder, "Plans"), (other, "Findings")] {
        sqlx::query("INSERT INTO document_containers (id,node_id,org_id,owner_id,created_by,name,created_at,updated_at)
          VALUES ($1,$2,$3,$4,$4,$5,$6,$6)")
            .bind(id).bind(node).bind(s.tree.org.base.id).bind(s.ada.id()).bind(name).bind(s.rig.world.now())
            .execute(s.rig.db.pool()).await.unwrap();
    }
    sqlx::query("UPDATE documents SET parent_container_id=$2 WHERE id=ANY($1)")
        .bind(vec![source, beside])
        .bind(folder)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE documents SET parent_container_id=$2 WHERE id=$1")
        .bind(sibling)
        .bind(other)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    set(
        &s,
        node,
        "related.weights",
        json!({"meaning":"0","place":"1","references":"0"}),
    )
    .await;
    let answer = related(&s, source, &s.cai.token).await;
    let position = |id| answer.items.iter().position(|item| item.id == id).unwrap();
    assert!(position(beside) < position(sibling));
    let beside = answer.items.iter().find(|item| item.id == beside).unwrap();
    assert_eq!(beside.place, Some(RelatedPlace::Folder));
    assert!(
        beside
            .context
            .iter()
            .any(|ancestor| ancestor.id == folder && ancestor.name.as_deref() == Some("Plans"))
    );
}

#[tokio::test]
async fn shared_workspace_file_references_match_encoded_paths_without_conflating_files_in_one_environment()
 {
    let s = Scene::new().await;
    let node = s.tree.topic.base.id;
    let source = document(&s, node, "Source", "A workspace reference", false).await;
    let inline = document(
        &s,
        node,
        "Inline reference",
        "The same workspace file",
        false,
    )
    .await;
    let link = document(
        &s,
        node,
        "Stored address",
        "The same encoded workspace file",
        false,
    )
    .await;
    let other = document(
        &s,
        node,
        "Another file",
        "Another path in that environment",
        false,
    )
    .await;
    for (document, path) in [
        (source, "design notes/说明.md"),
        (inline, "design notes/说明.md"),
        (other, "other.md"),
    ] {
        sqlx::query("UPDATE document_blocks SET content=$2 WHERE document_id=$1")
            .bind(document)
            .bind(
                json!([{"type":"ref","kind":"environment_file","id":s.on.environment,"path":path}]),
            )
            .execute(s.rig.db.pool())
            .await
            .unwrap();
    }
    let address = orch_documents::references::address(&orch_documents::references::Typed {
        kind: orch_api_contract::documents::InlineRefKind::EnvironmentFile,
        id: s.on.environment,
        path: Some("design notes/说明.md".into()),
        part: None,
    });
    sqlx::query("UPDATE document_blocks SET content=$2 WHERE document_id=$1")
        .bind(link)
        .bind(json!([{"type":"text","text":"A file","href":address}]))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    set(
        &s,
        node,
        "related.weights",
        json!({"meaning":"0","place":"0","references":"1"}),
    )
    .await;
    let answer = related(&s, source, &s.cai.token).await;
    let ids = answer
        .items
        .iter()
        .map(|item| item.id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids, std::collections::BTreeSet::from([inline, link]));
    assert!(
        answer
            .items
            .iter()
            .all(|item| item.references == Some(RelatedReference::Shared))
    );
}

#[tokio::test]
async fn a_related_direct_chat_uses_its_readers_current_title_and_stays_hidden_from_nonparticipants()
 {
    use orch_api_contract::chats::{OrgChatCreate, OrgChatCreateBody};
    use orch_api_contract::documents::{ReferencePreviewGet, ReferencePreviewQuery};
    let s = Scene::new().await;
    let chat = s
        .rig
        .world
        .app
        .call(OrgChatCreate, &[&s.tree.org.base.id])
        .token(&s.ada.token)
        .body(&OrgChatCreateBody {
            user_ids: vec![s.cai.id()],
            name: None,
        })
        .send()
        .await
        .ok();
    let source = document(
        &s,
        s.tree.topic.base.id,
        "Source",
        &format!("[A chat](/chats/{})", chat.base.id),
        false,
    )
    .await;
    set(
        &s,
        s.tree.topic.base.id,
        "related.weights",
        json!({"meaning":"0","place":"0","references":"1"}),
    )
    .await;
    let answer = related(&s, source, &s.cai.token).await;
    assert_eq!(answer.items.len(), 1);
    assert_eq!(answer.items[0].id, chat.base.id);
    let preview = s
        .rig
        .world
        .app
        .call(ReferencePreviewGet, &[])
        .token(&s.cai.token)
        .query(
            &serde_json::from_value::<ReferencePreviewQuery>(
                json!({"kind":"chat","id":chat.base.id}),
            )
            .unwrap(),
        )
        .send()
        .await
        .ok();
    assert_eq!(answer.items[0].name, preview.title);
    assert!(!answer.items[0].name.is_empty());
    assert!(related(&s, source, &s.eve.token).await.items.is_empty());
}

#[tokio::test]
async fn equal_ids_in_a_message_and_a_client_chosen_block_keep_independent_reference_edges() {
    let s = Scene::new().await;
    let node = s.tree.topic.base.id;
    let target = document(&s, node, "Target", "Shared by two independent parts", false).await;
    let chat = s.tree.topic.base.chat_id;
    let part = new_id();
    sqlx::query(
        "INSERT INTO chat_messages (id,chat_id,author_id,content,seq_created,seq_updated)
        VALUES ($1,$2,$3,$4,1,1)",
    )
    .bind(part)
    .bind(chat)
    .bind(s.ada.id())
    .bind(json!([{"type":"text","text":format!("[Target](/documents/{target})")}]))
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE chats SET next_seq=2,last_message_seq=1,last_message_at=$2 WHERE id=$1")
        .bind(chat)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let source=s.rig.world.app.call(NodeDocumentCreate,&[&node]).token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title:"Independent block".into(),markdown:None,access:None,
            blocks:Some(vec![serde_json::from_value(json!({"id":part,"type":"paragraph","props":{},"content":[{"type":"ref","kind":"document","id":target}]})).unwrap()]),
        }).send().await.ok().base.id;
    set(
        &s,
        node,
        "related.weights",
        json!({"meaning":"0","place":"0","references":"1"}),
    )
    .await;
    let first = related(&s, target, &s.cai.token).await;
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.id)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([source, chat])
    );
    sqlx::query("UPDATE document_blocks SET content='[]' WHERE id=$1")
        .bind(part)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let after = related(&s, target, &s.cai.token).await;
    assert_eq!(
        after.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [chat],
        "editing a block cannot remove a message's relation"
    );
}

async fn generation(s: &Scene) -> Id {
    let generation = new_id();
    sqlx::query("INSERT INTO search_generations (id,org_id,provider_id,model_name,dimensions,chunking,state,created_at)
        VALUES ($1,$2,$3,'related-fixture',3,'fixture','active',$4)")
        .bind(generation).bind(s.tree.org.base.id).bind(new_id()).bind(s.rig.world.now())
        .execute(s.rig.db.pool()).await.unwrap();
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::semantic::make_index(&mut conn, generation, 3)
        .await
        .unwrap();
    generation
}

async fn vector(s: &Scene, generation: Id, document: Id, vectors: &[Vec<f32>]) {
    let mut conn = s.rig.db.acquire().await.unwrap();
    let item = orch_db::queries::related::items(&mut conn, &[document])
        .await
        .unwrap()
        .remove(0);
    sqlx::query("INSERT INTO search_sources (generation_id,kind,source_id,title,org_id,marker,changed_at,chunks,tokens,bytes)
        VALUES ($1,'document',$2,$3,$4,$5,$6,$7,10,1000)")
        .bind(generation).bind(document).bind(&item.name).bind(item.org_id).bind(item.marker).bind(item.changed_at)
        .bind(i32::try_from(vectors.len()).unwrap()).execute(&mut *conn).await.unwrap();
    for (ordinal, vector) in vectors.iter().enumerate() {
        let chunk = new_id();
        sqlx::query("INSERT INTO search_chunks (id,generation_id,node_id,kind,source_id,org_id,ordinal,text_hash,tokens,bytes)
            VALUES ($1,$2,$3,'document',$4,$5,$6,$7,10,1000)")
            .bind(chunk).bind(generation).bind(item.node_id).bind(document).bind(item.org_id)
            .bind(i32::try_from(ordinal).unwrap()).bind(vec![1_u8; 32]).execute(&mut *conn).await.unwrap();
        orch_db::queries::semantic::put_vectors(
            &mut conn,
            generation,
            &[(chunk, vec![1_u8; 32], vector.clone())],
        )
        .await
        .unwrap();
    }
    orch_db::queries::semantic::mark_embedded(
        &mut conn,
        "document",
        document,
        item.marker,
        generation,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn shortening_recomputes_item_meaning_from_the_normalized_shortened_pieces() {
    let s = Scene::new().await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        return;
    }
    let source = document(&s, s.tree.topic.base.id, "Source", "Two pieces", false).await;
    let generation = generation(&s).await;
    vector(
        &s,
        generation,
        source,
        &[vec![1.0, 0.0, 1.0], vec![0.0, 1.0, 0.0]],
    )
    .await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::semantic::shorten(&mut conn, generation, 2, &json!([]))
        .await
        .unwrap();
    let centroid:Vec<f32>=sqlx::query_scalar("SELECT embedding::vector::real[] FROM search_item_vectors WHERE generation_id=$1 AND source_id=$2")
        .bind(generation).bind(source).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(centroid.len(), 2);
    assert!((centroid[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.001);
    assert!(
        (centroid[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.001,
        "truncating the old centroid would give unequal contributions to the two shortened pieces"
    );
    assert_eq!(s.rig.commands.issued_of("provider.embed"), 0);
}

#[tokio::test]
async fn a_blocked_meaning_index_keeps_place_results_within_the_configured_query_wait() {
    let s = Scene::new().await;
    let node = s.tree.topic.base.id;
    let source = document(&s, node, "Source", "A source", false).await;
    let beside = document(&s, node, "Beside", "A related neighbor", false).await;
    set(
        &s,
        s.tree.org.base.id,
        "search.semantic.enabled",
        json!(true),
    )
    .await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        return;
    }
    let generation = generation(&s).await;
    vector(&s, generation, source, &[vec![1.0, 0.0, 0.0]]).await;
    vector(&s, generation, beside, &[vec![1.0, 0.0, 0.0]]).await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let mut tuning = orch_settings::get::<Json>(
        &mut conn,
        orch_settings::Place::Node(s.tree.org.base.id),
        "search.semantic.tuning",
    )
    .await
    .unwrap()
    .unwrap();
    drop(conn);
    tuning["query_wait_secs"] = json!(1);
    set(&s, s.tree.org.base.id, "search.semantic.tuning", tuning).await;
    let mut locked = s.rig.db.begin().await.unwrap();
    sqlx::query("LOCK TABLE search_item_vectors IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locked)
        .await
        .unwrap();
    let start = std::time::Instant::now();
    let answer = related(&s, source, &s.cai.token).await;
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "the one-second optional lookup leaves its ordinary place result usable"
    );
    assert_eq!(answer.meaning, RelatedMeaning::Unavailable);
    assert!(
        answer
            .items
            .iter()
            .any(|item| item.id == beside && item.related_by.contains(&RelatedBy::Place))
    );
    locked.rollback().await.unwrap();
}

#[tokio::test]
async fn meaning_uses_completed_item_vectors_from_only_the_current_generation_and_never_calls_a_model()
 {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let source = document(&s, topic, "Source", "First half and second half", false).await;
    let similar = document(
        &s,
        s.tree.org.base.id,
        "Similar meaning",
        "In another place",
        false,
    )
    .await;
    let later = document(
        &s,
        topic,
        "Next generation",
        "Another embedding space",
        false,
    )
    .await;
    let hidden = document(&s, topic, "Hidden nearest", "Private", true).await;
    sqlx::query("UPDATE document_blocks SET content=$2 WHERE document_id=$1")
        .bind(source)
        .bind(json!([{"type":"ref","kind":"document","id":later}]))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    set(
        &s,
        topic,
        "related.weights",
        json!({"meaning":"1","place":"0","references":"0"}),
    )
    .await;
    set(
        &s,
        s.tree.org.base.id,
        "search.semantic.enabled",
        json!(true),
    )
    .await;
    if s.rig.engine.semantic_available().await.unwrap().is_err() {
        assert_eq!(
            related(&s, source, &s.cai.token).await.meaning,
            RelatedMeaning::Unavailable
        );
        return;
    }
    let old = generation(&s).await;
    vector(&s, old, source, &[vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]]).await;
    vector(&s, old, similar, &[vec![1.0, 1.0, 0.0]]).await;
    vector(&s, old, later, &[vec![0.0, 0.0, 1.0]]).await;
    vector(&s, old, hidden, &[vec![1.0, 1.0, 0.0]]).await;
    let first = related(&s, source, &s.cai.token).await;
    assert_eq!(first.meaning, RelatedMeaning::Ready);
    assert_eq!(
        first.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        [similar]
    );
    assert_eq!(first.items[0].related_by, [RelatedBy::Meaning]);
    let mut conn = s.rig.db.acquire().await.unwrap();
    let mut tuning = orch_settings::get::<Json>(
        &mut conn,
        orch_settings::Place::Node(s.tree.org.base.id),
        "search.semantic.tuning",
    )
    .await
    .unwrap()
    .unwrap();
    drop(conn);
    tuning["exact_filter_candidates"] = json!(0);
    set(&s, s.tree.org.base.id, "search.semantic.tuning", tuning).await;
    assert_eq!(
        related(&s, source, &s.cai.token).await.items,
        first.items,
        "the graph and bounded exact source use the same access and scoring rules"
    );
    set(
        &s,
        topic,
        "related.weights",
        json!({"meaning":"0.6","place":"0.25","references":"0.15"}),
    )
    .await;
    let combined = related(&s, source, &s.cai.token).await;
    let position = |id| {
        combined
            .items
            .iter()
            .position(|item| item.id == id)
            .unwrap()
    };
    assert!(
        position(similar) < position(later),
        "meaning balances a closer directly referenced document"
    );
    assert_eq!(
        combined
            .items
            .iter()
            .find(|item| item.id == later)
            .unwrap()
            .related_by,
        [RelatedBy::Place, RelatedBy::References]
    );
    assert!(!combined.items.iter().any(|item| item.id == hidden));
    set(
        &s,
        topic,
        "related.weights",
        json!({"meaning":"0","place":"1","references":"0"}),
    )
    .await;
    let by_place = related(&s, source, &s.cai.token).await;
    assert!(
        by_place
            .items
            .iter()
            .position(|item| item.id == later)
            .unwrap()
            < by_place
                .items
                .iter()
                .position(|item| item.id == similar)
                .unwrap()
    );
    set(
        &s,
        topic,
        "related.place_scores",
        json!({"folder":"1","topic":"0","project":"0","org":"1"}),
    )
    .await;
    set(&s, topic, "related.count", json!(1)).await;
    let custom = related(&s, source, &s.cai.token).await;
    assert_eq!(custom.items.len(), 1);
    assert_eq!(
        custom.items[0].place,
        Some(RelatedPlace::Org),
        "place steps and count are inherited settings, not fixed rank constants"
    );
    set(
        &s,
        topic,
        "related.weights",
        json!({"meaning":"1","place":"0","references":"0"}),
    )
    .await;
    let centroid: Vec<f32> = sqlx::query_scalar("SELECT embedding::vector::real[] FROM search_item_vectors WHERE generation_id=$1 AND source_id=$2")
        .bind(old).bind(source).fetch_one(s.rig.db.pool()).await.unwrap();
    assert!((centroid[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.001);
    assert!((centroid[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.001);
    sqlx::query("UPDATE search_generations SET state='retired' WHERE id=$1")
        .bind(old)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let new = generation(&s).await;
    vector(&s, new, source, &[vec![0.0, 0.0, 1.0]]).await;
    vector(&s, new, later, &[vec![0.0, 0.0, 1.0]]).await;
    let switched = related(&s, source, &s.cai.token).await;
    assert_eq!(
        switched
            .items
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        [later],
        "retired generation vectors never participate"
    );
    sqlx::query(
        "UPDATE search_sources SET embedded_for=NULL WHERE generation_id=$1 AND source_id=$2",
    )
    .bind(new)
    .bind(source)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        related(&s, source, &s.cai.token).await.meaning,
        RelatedMeaning::NotReady
    );
    assert_eq!(s.rig.commands.issued_of("provider.embed"), 0);
}

#[tokio::test]
async fn related_pages_are_bounded_complete_and_retire_a_changed_ranking() {
    use orch_api_contract::common::PageQuery;
    use std::collections::HashSet;
    let s = Scene::new().await;
    let node = s.tree.topic.base.id;
    let source = document(&s, node, "Paged source", "A source", false).await;
    let mut expected = HashSet::new();
    for number in 0..20 {
        expected.insert(document(&s, node, &format!("Paged peer {number}"), "A peer", false).await);
    }
    set(
        &s,
        node,
        "related.weights",
        json!({"meaning":"0","place":"1","references":"0"}),
    )
    .await;
    set(&s, node, "related.count", json!(17)).await;
    let mut cursor = None;
    let mut seen = HashSet::new();
    let mut first = None;
    for page in 0..7 {
        let answer = s
            .rig
            .world
            .app
            .call(DocumentRelatedList, &[&source])
            .token(&s.cai.token)
            .query(&PageQuery {
                limit: Some(3),
                cursor,
            })
            .send()
            .await
            .ok();
        assert!(answer.items.len() <= 3);
        for item in answer.items {
            assert!(seen.insert(item.id), "a continuation duplicated an item");
        }
        if page == 0 {
            first = answer.next_cursor.clone();
            assert!(first.is_some());
        }
        cursor = answer.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, expected,
        "the page default never hides the remaining peers"
    );
    assert!(cursor.is_none());
    set(&s, node, "related.count", json!(16)).await;
    let stale = s
        .rig
        .world
        .app
        .call(DocumentRelatedList, &[&source])
        .token(&s.cai.token)
        .query(&PageQuery {
            limit: Some(3),
            cursor: first,
        })
        .send_bytes()
        .await;
    assert_eq!(
        stale.status, 400,
        "an old ranking cannot silently skip new items"
    );
}
