//! Durable copy replay and an observed late material failure use the actual shared producer.

use super::*;

/// Limit rollback-surviving probes to this copy's destination and reset their transaction-local order marker.
async fn observe_destination(tx: &mut orch_db::Tx, node: Id) -> TestResult {
    sqlx::query("SELECT set_config('orch.test.document_copy_node',$1,true),set_config('orch.test.document_copy_later','',true)")
        .bind(node.to_string()).execute(&mut **tx).await?;
    Ok(())
}

#[tokio::test]
async fn a_committed_agent_copy_replayed_in_a_fresh_engine_keeps_every_identity_history_audit_and_pin()
-> TestResult {
    use orch_blob_contract::store::{BlobStore, read_all};
    let area = Area::new(true).await?;
    let source = area.folder("research/").await?;
    area.folder("research/notes/").await?;
    let first = area
        .document(
            "research/Plan.md",
            "SOURCE-HISTORY-ONLY: this earlier version is not a copied history.",
        )
        .await?;
    let second = area
        .document("research/notes/Notes.md", "A second current document.")
        .await?;
    let mut history = area.world.pool.begin().await?;
    let old = documents::lock(&mut history, first)
        .await?
        .ok_or("source absent before history cut")?;
    documents::history::cut(
        &mut history,
        &old,
        VersionCause::Rewrite,
        area.world.clock.now(),
    )
    .await?;
    history.commit().await?;
    let old = documents::get(&mut *area.world.pool.acquire().await?, first)
        .await?
        .ok_or("source absent before rewrite")?;
    ok(call(
        &area.world,
        area.agent,
        OrchTool::EditDocument,
        json!({"command":"rewrite","path":format!("id:{first}"),
        "seq":old.doc_seq,"markdown":"Current copied material."}),
    )
    .await);
    let attached = attach(&area.world, first).await?;
    let (old_file, hash) = (attached.file, attached.hash);
    let original_blocks = area.world.blocks_of(first).await;
    let mut copy = request(
        domain::Command::Copy,
        vec![domain::Item::Container(source)],
        Some(domain::Directory::NodeDocs(area.topic)),
        "durable-agent-copy",
    );
    copy.name = Some("research-copy".into());
    let mut tx = area.world.pool.begin().await?;
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let receipt = domain::execute_in(
        &mut tx,
        domain::Actor::Agent { thread: area.agent },
        &copy,
        None,
        now,
        options,
    )
    .await?;
    tx.commit().await?;
    // Reconstruct exactly the committed-producer / missing-tool-reply checkpoint. This is
    // not a killed process: the first producer deliberately never installs an in-memory answer.
    assert_eq!(receipt.entries.len(), 4);
    assert_eq!(receipt.copied.len(), 4);
    let copied_first = receipt
        .copied
        .iter()
        .find(|pair| pair.source == domain::Item::Document(first))
        .ok_or("first copy map absent")?
        .destination
        .id();
    let copied_second = receipt
        .copied
        .iter()
        .find(|pair| pair.source == domain::Item::Document(second))
        .ok_or("second copy map absent")?
        .destination
        .id();
    assert_ne!(copied_first, first);
    assert_ne!(copied_second, second);
    let fresh_files =
        documents::files(&mut *area.world.pool.acquire().await?, copied_first).await?;
    assert_eq!(fresh_files.len(), 1);
    assert_ne!(fresh_files[0].id, old_file);
    let blocks =
        documents::live_blocks(&mut *area.world.pool.acquire().await?, copied_first).await?;
    assert_eq!(blocks.len(), original_blocks.len());
    assert!(
        blocks
            .iter()
            .all(|block| original_blocks.iter().all(|(old, _)| *old != block.id))
    );
    assert!(
        blocks
            .iter()
            .any(|block| block.props["file_id"] == json!(fresh_files[0].id))
    );
    let old_history: Vec<Json> = sqlx::query_scalar(
        "SELECT COALESCE(whole, change) FROM document_versions WHERE document_id=$1 ORDER BY version",
    )
    .bind(first)
    .fetch_all(area.world.pool.pool())
    .await?;
    assert!(
        old_history
            .iter()
            .any(|snapshot| snapshot.to_string().contains("SOURCE-HISTORY-ONLY"))
    );
    let new_history: Vec<Json> = sqlx::query_scalar(
        "SELECT COALESCE(whole, change) FROM document_versions WHERE document_id=$1 ORDER BY version",
    )
    .bind(copied_first)
    .fetch_all(area.world.pool.pool())
    .await?;
    assert!(
        new_history
            .iter()
            .all(|snapshot| !snapshot.to_string().contains("SOURCE-HISTORY-ONLY"))
    );
    assert_eq!(pins(&area.world, hash).await?, 2);
    let before = state(&area.world, &[area.topic]).await?;
    area.world.restart_api().await;
    area.world.engine().attach_blobs(attached.store.clone());
    let turn: Id = area
        .world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            area.agent,
        )
        .await;
    let context = CallContext {
        machine: area.world.machine_id,
        thread: area.agent,
        turn,
        call_ref: copy.request_ref.clone(),
        attempt: None,
        tool: orch_agent_contract::events::OrchTool::ManageShared,
        arguments: json!({"command":"copy","paths":[area.path("research/")],"to":area.root,"name":"research-copy"}),
    };
    area.world.engine().answer_tool_call(&context).await?;
    let answer = shared::answer_to(&area.world, area.agent, &copy.request_ref).await;
    let answer = ok(answer);
    assert_eq!(answer["done"], true);
    for pair in &receipt.copied {
        assert!(
            answer["results"]
                .as_array()
                .ok_or("copy result list absent")?
                .iter()
                .any(|entry| entry["id"] == json!(pair.destination.id()))
        );
    }
    assert_eq!(
        state(&area.world, &[area.topic]).await?,
        before,
        "fresh-engine replay changes no material, pages, history, audit, receipt, tree stamp or blob reference"
    );
    assert_eq!(area.world.blocks_of(first).await, original_blocks);
    assert_eq!(
        read_all(attached.store.get(&hash, None).await?.body)
            .await?
            .as_ref(),
        b"\0\xffretained document-copy bytes\r\n"
    );
    let mut tx = area.world.pool.begin().await?;
    let saved = documents::containers::operation(&mut tx, "agent", area.agent, &copy.request_ref)
        .await?
        .ok_or("durable receipt absent")?;
    let saved: domain::Receipt = serde_json::from_value(saved.result)?;
    assert_eq!(saved, receipt, "the exact original-to-fresh map is durable");
    let mut changed = copy.clone();
    changed.name = Some("different-arguments".into());
    assert!(matches!(
        domain::execute_in(
            &mut tx,
            domain::Actor::Agent { thread: area.agent },
            &changed,
            None,
            Time::new(area.world.clock.now()),
            options
        )
        .await,
        Err(domain::Error::Conflict)
    ));
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    let other = area.world.thread("A separate copying agent").await;
    for permission in ["doc.view", "doc.edit"] {
        share(
            &area.world,
            "node",
            area.topic,
            "live",
            permission,
            "any",
            other,
        )
        .await;
    }
    let mut separate = copy.clone();
    separate.name = Some("research-copy-other".into());
    let mut tx = area.world.pool.begin().await?;
    let other_receipt = domain::execute_in(
        &mut tx,
        domain::Actor::Agent { thread: other },
        &separate,
        None,
        Time::new(area.world.clock.now()),
        options,
    )
    .await?;
    tx.commit().await?;
    assert_ne!(other_receipt.operation, receipt.operation);
    assert!(other_receipt.copied.iter().all(|pair| {
        receipt
            .copied
            .iter()
            .all(|old| old.destination != pair.destination)
    }));
    assert_eq!(
        pins(&area.world, hash).await?,
        3,
        "the separate actor makes precisely its own new file reference"
    );
    Ok(())
}

#[tokio::test]
async fn a_late_second_document_limit_rolls_back_earlier_folder_content_file_pin_history_grants_audit_and_receipt()
-> TestResult {
    let area = Area::new(true).await?;
    let source = area.folder("research/").await?;
    let a = area
        .document("research/One.md", "An initially small source.")
        .await?;
    let b = area
        .document("research/Two.md", "Another initially small source.")
        .await?;
    // The actual domain sorts sibling document identities. The first must really install;
    // the second must fail after that installation, rather than during a preflight refusal.
    let (first, last) = if a < b { (a, b) } else { (b, a) };
    let row = documents::get(&mut *area.world.pool.acquire().await?, last)
        .await?
        .ok_or("large source absent")?;
    let large = (0..48)
        .map(|index| format!("Paragraph {index}: {}", "x".repeat(1_600)))
        .collect::<Vec<_>>()
        .join("\n\n");
    ok(call(
        &area.world,
        area.agent,
        OrchTool::EditDocument,
        json!({"command":"rewrite","path":format!("id:{last}"),"seq":row.doc_seq,"markdown":large}),
    )
    .await);
    let attached = attach(&area.world, first).await?;
    let hash = attached.hash;
    let good = documents::get(&mut *area.world.pool.acquire().await?, first)
        .await?
        .ok_or("first source absent")?;
    let oversized = documents::get(&mut *area.world.pool.acquire().await?, last)
        .await?
        .ok_or("last source absent")?;
    assert!(good.bytes < 65_536 && good.block_count < 5_000);
    assert!(
        oversized.bytes > 65_536 && oversized.block_count < 5_000,
        "only the later source exceeds the selected destination content bound"
    );
    let files:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM document_files WHERE document_id=$1),(SELECT count(*) FROM document_files WHERE document_id=$2)")
        .bind(first).bind(last).fetch_one(area.world.pool.pool()).await?;
    assert_eq!(
        files,
        (1, 0),
        "only the first copy source carries the file that identifies its block-insert probe"
    );
    let destination = area.world.topic("Small destination").await;
    share(
        &area.world,
        "node",
        destination,
        "live",
        "doc.edit",
        "any",
        area.agent,
    )
    .await;
    area.world
        .set(destination, "docs.max_bytes", json!(65_536))
        .await;
    let mut copy = request(
        domain::Command::Copy,
        vec![domain::Item::Container(source)],
        Some(domain::Directory::NodeDocs(destination)),
        "late-failure-copy",
    );
    copy.name = Some("copied-research".into());
    // Disposable native sequences observe actual writes even after rollback. The copy producer
    // installs a document's files before its blocks. That distinguishes the first document from
    // the fileless later document, whose blocks are also inserted before its tally rejects them.
    // The transaction-local marker observes their order; no trigger injects a failure.
    sqlx::raw_sql("CREATE SEQUENCE document_copy_file_checkpoint;
        CREATE SEQUENCE document_copy_block_checkpoint;
        CREATE SEQUENCE document_copy_first_block_checkpoint;
        CREATE SEQUENCE document_copy_later_block_checkpoint;
        CREATE SEQUENCE document_copy_wrong_order_checkpoint;
        CREATE FUNCTION observe_document_copy_file() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM documents d WHERE d.id=NEW.document_id
                AND d.node_id::text=current_setting('orch.test.document_copy_node',true)) THEN RETURN NEW; END IF;
            PERFORM nextval('document_copy_file_checkpoint'); RETURN NEW; END; $$;
        CREATE FUNCTION observe_document_copy_block() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM documents d WHERE d.id=NEW.document_id
                AND d.node_id::text=current_setting('orch.test.document_copy_node',true)) THEN RETURN NEW; END IF;
            IF EXISTS (SELECT 1 FROM document_files f WHERE f.document_id=NEW.document_id) THEN
                IF current_setting('orch.test.document_copy_later',true)='yes' THEN
                    PERFORM nextval('document_copy_wrong_order_checkpoint');
                END IF;
                PERFORM nextval('document_copy_first_block_checkpoint');
            ELSE
                PERFORM set_config('orch.test.document_copy_later','yes',true);
                PERFORM nextval('document_copy_later_block_checkpoint');
            END IF;
            PERFORM nextval('document_copy_block_checkpoint'); RETURN NEW; END; $$;
        CREATE TRIGGER observe_document_copy_file AFTER INSERT ON document_files FOR EACH ROW EXECUTE FUNCTION observe_document_copy_file();
        CREATE TRIGGER observe_document_copy_block AFTER INSERT ON document_blocks FOR EACH ROW EXECUTE FUNCTION observe_document_copy_block();")
        .execute(area.world.pool.pool()).await?;
    let before = state(&area.world, &[area.topic, destination]).await?;
    let mut tx = area.world.pool.begin().await?;
    let actor = domain::Actor::Agent { thread: area.agent };
    let options = domain::Options::for_engine(&area.world.engine());
    observe_destination(&mut tx, destination).await?;
    let result = domain::execute_in(
        &mut tx,
        actor,
        &copy,
        None,
        Time::new(area.world.clock.now()),
        options,
    )
    .await;
    assert!(
        matches!(result, Err(domain::Error::Limit)),
        "the later native content batch must exceed the destination limit"
    );
    // Commit the caller after the refusal: the producer's own savepoint must have undone everything.
    tx.commit().await?;
    let checkpoint: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM document_copy_file_checkpoint")
            .fetch_one(area.world.pool.pool())
            .await?;
    assert_eq!(
        checkpoint,
        (1, true),
        "the first copied attachment was installed before the later refusal"
    );
    let first_blocks: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM document_copy_first_block_checkpoint")
            .fetch_one(area.world.pool.pool())
            .await?;
    assert_eq!(
        first_blocks,
        (i64::from(good.block_count), true),
        "the first document's exact content was inserted before the later refusal"
    );
    let later_blocks: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM document_copy_later_block_checkpoint")
            .fetch_one(area.world.pool.pool())
            .await?;
    assert_eq!(
        later_blocks,
        (i64::from(oversized.block_count), true),
        "the later document reaches the actual content inserts before its byte tally refuses it"
    );
    let wrong_order: bool =
        sqlx::query_scalar("SELECT is_called FROM document_copy_wrong_order_checkpoint")
            .fetch_one(area.world.pool.pool())
            .await?;
    assert!(
        !wrong_order,
        "every first-document block precedes the later document's inserts"
    );
    let blocks: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM document_copy_block_checkpoint")
            .fetch_one(area.world.pool.pool())
            .await?;
    assert_eq!(
        blocks,
        (
            i64::from(good.block_count) + i64::from(oversized.block_count),
            true
        ),
        "the combined probe includes both the accepted first batch and the refused later batch"
    );
    assert_eq!(
        state(&area.world, &[area.topic, destination]).await?,
        before
    );
    assert_eq!(
        pins(&area.world, hash).await?,
        1,
        "rollback retained only the original file reference"
    );
    area.world
        .set(destination, "docs.max_bytes", json!(2_097_152))
        .await;
    let mut tx = area.world.pool.begin().await?;
    observe_destination(&mut tx, destination).await?;
    let success = domain::execute_in(
        &mut tx,
        actor,
        &copy,
        None,
        Time::new(area.world.clock.now()),
        options,
    )
    .await?;
    tx.commit().await?;
    assert_eq!(success.entries.len(), 3);
    assert_eq!(pins(&area.world, hash).await?, 2);
    let checkpoint: i64 =
        sqlx::query_scalar("SELECT last_value FROM document_copy_file_checkpoint")
            .fetch_one(area.world.pool.pool())
            .await?;
    assert_eq!(
        checkpoint, 2,
        "the corrected retry installs exactly one successful copy file"
    );
    let first_after: i64 =
        sqlx::query_scalar("SELECT last_value FROM document_copy_first_block_checkpoint")
            .fetch_one(area.world.pool.pool())
            .await?;
    let later_after: i64 =
        sqlx::query_scalar("SELECT last_value FROM document_copy_later_block_checkpoint")
            .fetch_one(area.world.pool.pool())
            .await?;
    assert_eq!(first_after, 2 * i64::from(good.block_count));
    assert_eq!(later_after, 2 * i64::from(oversized.block_count));
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT is_called FROM document_copy_wrong_order_checkpoint"
        )
        .fetch_one(area.world.pool.pool())
        .await?
    );
    let accepted = state(&area.world, &[area.topic, destination]).await?;
    let mut tx = area.world.pool.begin().await?;
    observe_destination(&mut tx, destination).await?;
    let repeat = domain::execute_in(
        &mut tx,
        actor,
        &copy,
        None,
        Time::new(area.world.clock.now()),
        options,
    )
    .await?;
    tx.commit().await?;
    assert_eq!(repeat, success);
    assert_eq!(
        state(&area.world, &[area.topic, destination]).await?,
        accepted
    );
    let replay_probes:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT last_value FROM document_copy_file_checkpoint),
        (SELECT last_value FROM document_copy_block_checkpoint),(SELECT last_value FROM document_copy_first_block_checkpoint),
        (SELECT last_value FROM document_copy_later_block_checkpoint)").fetch_one(area.world.pool.pool()).await?;
    assert_eq!(
        replay_probes,
        (
            2,
            2 * (i64::from(good.block_count) + i64::from(oversized.block_count)),
            first_after,
            later_after
        ),
        "replaying the successful receipt inserts no file or block in either document"
    );
    Ok(())
}
