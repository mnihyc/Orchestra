//! The page readers and what is written from them: against the whole readings they replace, on small threads, and at the size
//! of a history of 200,000 turns made by SQL.

use super::*;
use crate::fork_history::{self, cut};
use crate::test_common as common;
use crate::written::{self, Detail};
use orch_authz::Caller;
use orch_db::queries::{thread_history, thread_history_cuts as cuts, threads};
use orch_types::{Json, new_id};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How many turns the history at size has: the figure the owner's review asked for.
const SCALE_TURNS: i64 = 200_000;

/// Counts the statements that run in a piece of work: sqlx tells of each one as an event while the task that makes it is polled.
/// One subscriber is installed for the whole test binary; it counts the events of the task that is counting, so that tests that
/// run side by side, and the tasks of the world they stand on, do not count in each other's.
struct Statements;

tokio::task_local! {
    static COUNTING: std::sync::Arc<std::sync::atomic::AtomicUsize>;
}

impl Statements {
    /// Runs `work` and tells how many statements it ran.
    async fn of<T>(work: impl std::future::Future<Output = T>) -> (T, usize) {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = tracing::subscriber::set_global_default(Statements);
        });
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let done = COUNTING.scope(std::sync::Arc::clone(&count), work).await;
        (done, count.load(std::sync::atomic::Ordering::Relaxed))
    }
}

impl tracing::Subscriber for Statements {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            let _ =
                COUNTING.try_with(|count| count.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// A thread with `turns` real turns: each is asked a different thing and answered a different thing.
async fn conversation(world: &common::World, turns: usize) -> Proof<Id> {
    let thread = world.thread("A history read in pages").await;
    for number in 0..turns {
        world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                common::queued(&format!("ask {number}")),
            )
            .await?;
        world
            .run_turn(thread, number, &format!("answer {number}"))
            .await;
        common::eventually("the terminal turn is accounted", || async {
            let Ok(mut conn) = world.pool.acquire().await else {
                return false;
            };
            !threads::turn_end_work_pending(&mut conn, thread)
                .await
                .unwrap_or(true)
        })
        .await;
        // SQL-grown copies must begin with a fully settled template, including its file work.
        // Otherwise the fixture also copies pending follow-up into every synthetic turn.
        world.wait_followed_up(thread).await;
    }
    Ok(thread)
}

/// Captures the whole history of a thread as a cut.
async fn captured(world: &common::World, thread: Id) -> Proof<cuts::Cut> {
    let mut conn = world.pool.acquire().await?;
    let row = threads::thread(&mut conn, thread)
        .await?
        .ok_or("the thread is absent")?;
    let last = thread_history::last_turn(&mut conn, thread, row.active_continuation_id).await?;
    Ok(cuts::capture(
        &mut conn,
        &cuts::Capture {
            // A cut owned by a thread is named by it.
            id: thread,
            thread,
            view: row.active_continuation_id,
            job: None,
            owner_thread: Some(thread),
            segment: row.current_segment_id,
            execution_revision: row.execution_revision,
            range: Some((1, i32::try_from(last)?)),
            at: world.clock.now(),
        },
    )
    .await?)
}

/// The readings this part replaced, kept as they were so that what replaces them can be compared with them on a small
/// thread: they read the whole history and make a query for each turn.
mod reference {
    use super::*;
    use crate::views::{People, item_view, turn_view};
    use std::collections::HashMap;

    /// The written turns of a cut and their index lines, as the whole reading made them.
    pub(super) async fn cut_rendered(
        conn: &mut orch_db::Conn,
        cut: &cuts::Cut,
        detail: Detail,
    ) -> Proof<(Vec<i64>, Vec<String>, Vec<String>)> {
        let turns = cuts::turns(
            conn,
            cut.id,
            i64::from(cut.first_turn),
            i64::from(cut.last_turn),
        )
        .await?;
        let agents: HashMap<Id, String> = cuts::metadata(conn, cut.id, "segment")
            .await?
            .into_iter()
            .map(|body| {
                Ok((
                    serde_json::from_value(body["id"].clone())?,
                    body["captured_agent_name"]
                        .as_str()
                        .unwrap_or("Agent")
                        .to_owned(),
                ))
            })
            .collect::<Result<_, serde_json::Error>>()?;
        let mut beside = written::Beside::default();
        for body in cuts::metadata(conn, cut.id, "approval").await? {
            let id = serde_json::from_value(body["id"].clone())?;
            beside.approvals.insert(
                id,
                written::ApprovalFacts {
                    title: body["title"].as_str().unwrap_or_default().to_owned(),
                    decided_by: body["captured_decided_by"].as_str().map(str::to_owned),
                },
            );
        }
        let (mut previews, mut texts) = (Vec::new(), Vec::new());
        let mut previous = 0;
        for turn in &turns {
            let through = cuts::turn_high(conn, cut.id, i64::from(turn.ordinal))
                .await?
                .unwrap_or(previous);
            let mut items = cuts::items_of_turn(conn, cut.id, turn.id).await?;
            let loose = cuts::items_between(conn, cut.id, previous, through, Some("main")).await?;
            items.extend(loose.into_iter().filter(|item| item.turn_id.is_none()));
            previous = through;
            items.sort_by_key(|item| item.seq_created);
            items.retain(|item| item.kind != "reasoning");
            let people = People::load(
                conn,
                items
                    .iter()
                    .filter_map(|item| item.author_id)
                    .chain(turn.interrupted_by),
            )
            .await?;
            let typed_turn = turn_view(turn, "", &people)?;
            let typed_items: Vec<_> = items.iter().map(|item| item_view(item, &people)).collect();
            for item in &typed_items {
                if let orch_api_contract::threads::ItemPayload::AgentMessage(message) =
                    &item.payload
                    && item.stream != "main"
                {
                    beside
                        .sub_agents
                        .insert(item.stream.clone(), message.text.clone());
                }
            }
            let main: Vec<_> = typed_items
                .iter()
                .filter(|item| item.stream == "main")
                .collect();
            let first = items
                .iter()
                .find(|item| item.kind == "user_message" && item.turn_id == Some(turn.id));
            let author = first.and_then(|item| item.author_id);
            let author = if let Some(id) = author {
                threads::people(conn, &[id])
                    .await?
                    .into_iter()
                    .next()
                    .map(|person| person.display_name)
            } else {
                None
            };
            let asked = first
                .into_iter()
                .flat_map(|item| item.payload["parts"].as_array())
                .flatten()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            previews.push(format!(
                "Turn {} · {} · {} · {} · {}",
                turn.ordinal,
                turn.started_at.unwrap_or(turn.created_at).to_rfc3339(),
                author.as_deref().unwrap_or("Orch"),
                asked
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(written::BRIEF_ASKED_CHARS)
                    .collect::<String>(),
                turn.state
            ));
            texts.push(written::turn_text(
                &written::Part {
                    turn: &typed_turn,
                    agent: agents.get(&turn.segment_id).map_or("Agent", String::as_str),
                    items: main,
                },
                &beside,
                detail,
            ));
        }
        Ok((
            turns.iter().map(|turn| i64::from(turn.ordinal)).collect(),
            previews,
            texts,
        ))
    }

    /// The transcript of a cut as the whole reading made it.
    pub(super) async fn cut_transcript(conn: &mut orch_db::Conn, cut: &cuts::Cut) -> Proof<String> {
        let (turns, previews, texts) = cut_rendered(conn, cut, Detail::Full).await?;
        let header = "# Conversation index\n\n";
        let mut line = header.lines().count() + previews.len() + 2;
        let index = previews
            .iter()
            .zip(&texts)
            .map(|(preview, text)| {
                let entry = format!("- {preview} · line {line}");
                line += text.lines().count() + 1;
                entry
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!turns.is_empty(), "the reference needs a thread with turns");
        Ok(format!("{header}{index}\n\n{}", texts.join("\n")))
    }

    /// The carry of a cut as the whole reading chose it: the newest turns that fit.
    pub(super) async fn cut_recent(
        conn: &mut orch_db::Conn,
        cut: &cuts::Cut,
        detail: Detail,
        budget: Option<usize>,
        last: Option<usize>,
    ) -> Proof<Option<(i64, i64, String)>> {
        let (ordinals, _, texts) = cut_rendered(conn, cut, detail).await?;
        let mut taken: Vec<usize> = Vec::new();
        let mut used = 0usize;
        for index in (0..texts.len()).rev() {
            if last.is_some_and(|last| taken.len() >= last) {
                break;
            }
            let size = texts[index].len() + 1;
            if budget.is_some_and(|budget| used + size > budget) {
                if taken.is_empty() {
                    taken.push(index);
                }
                break;
            }
            used += size;
            taken.push(index);
        }
        taken.reverse();
        let (Some(&first), Some(&latest)) = (taken.first(), taken.last()) else {
            return Ok(None);
        };
        let mut text = taken
            .iter()
            .map(|&index| texts[index].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(budget) = budget
            && text.len() > budget
        {
            let mut end = budget;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("\n\n(This turn is cut here; the rest of it is in the transcript file.)");
        }
        Ok(Some((ordinals[first], ordinals[latest], text)))
    }

    /// The indexed transcript of the selected history as the whole reading made it: one reading of the conversation for
    /// each turn.
    pub(super) async fn live_transcript(
        conn: &mut orch_db::Conn,
        row: &threads::ThreadRow,
        from: Option<i64>,
        through: Option<i64>,
    ) -> Proof<(String, i64)> {
        let thread = row.node_id;
        let view = row.active_continuation_id;
        let last = thread_history::last_turn(conn, thread, view).await?;
        let mut turns = thread_history::turns_between(
            conn,
            thread,
            view,
            from.unwrap_or(1),
            through.unwrap_or(last).min(last),
        )
        .await?;
        if through.is_none() {
            let last_completed = turns
                .iter()
                .filter(|turn| turn.state == "completed")
                .map(|turn| turn.ordinal)
                .max()
                .unwrap_or(0);
            turns.retain(|turn| turn.ordinal <= last_completed);
        }
        let mut parts = Vec::new();
        let mut previews = Vec::new();
        for turn in &turns {
            let text = written::read_conversation_view(
                conn,
                thread,
                view,
                (Some(i64::from(turn.ordinal)), Some(i64::from(turn.ordinal))),
                Detail::Full,
                None,
                None,
            )
            .await?
            .ok_or("a recorded source turn is unavailable")?
            .text;
            let first = thread_history::items_of_turns(conn, thread, view, &[turn.id])
                .await?
                .into_iter()
                .find(|item| item.kind == "user_message");
            let author = match first.as_ref().and_then(|item| item.author_id) {
                Some(id) => threads::people(conn, &[id])
                    .await?
                    .into_iter()
                    .next()
                    .map(|person| person.display_name),
                None => None,
            };
            let payload = first.map_or(Json::Null, |item| item.payload);
            let preview = payload["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part.get("text").and_then(Json::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            previews.push(format!(
                "Turn {} · {} · {} · {} · {}",
                turn.ordinal,
                turn.started_at.unwrap_or(turn.created_at).to_rfc3339(),
                author.as_deref().unwrap_or("Orch"),
                preview
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(200)
                    .collect::<String>(),
                turn.state
            ));
            parts.push(text);
        }
        let header = "# Conversation index\n\n";
        let mut line = header.lines().count() + previews.len() + 2;
        let index = previews
            .iter()
            .zip(&parts)
            .map(|(preview, text)| {
                let entry = format!("- {preview} · line {line}");
                line += text.lines().count() + 1;
                entry
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok((
            format!("{header}{index}\n\n{}", parts.join("\n")),
            i64::try_from(turns.len())?,
        ))
    }
}

async fn stored_text(world: &common::World, artifact: &cut::TranscriptFile) -> Proof<String> {
    let store = world
        .engine()
        .blobs()
        .ok_or("no store of content is attached")?;
    let bytes =
        orch_blob_contract::store::read_all(store.get(&artifact.blob, None).await?.body).await?;
    Ok(String::from_utf8(bytes.to_vec())?)
}

#[tokio::test]
async fn a_cut_counts_an_adopted_item_after_its_live_row_is_removed() -> Proof {
    let world = common::World::new().await;
    let thread = conversation(&world, 1).await?;
    let adopted = new_id();
    let mut conn = world.pool.acquire().await?;
    let added = sqlx::query(
        "INSERT INTO items SELECT copied.* FROM (
            SELECT i.* FROM items i WHERE i.thread_id=$1 AND i.kind='agent_message' LIMIT 1
         ) seed CROSS JOIN LATERAL jsonb_populate_record(NULL::items,to_jsonb(seed)||jsonb_build_object(
            'id',$2::uuid,'turn_id',NULL,'before_orch',true,'seq_created',1,'seq_updated',1,
            'native_ref',NULL,'payload',jsonb_build_object('text','Adopted before Orch'))) copied",
    )
    .bind(thread)
    .bind(adopted)
    .execute(&mut *conn)
    .await?;
    assert_eq!(added.rows_affected(), 1);
    drop(conn);
    let cut = captured(&world, thread).await?;
    let mut conn = world.pool.acquire().await?;
    let before = cuts::counts(&mut conn, cut.id).await?;
    assert_eq!(before.items.get("agent_message"), Some(&2));
    sqlx::query("DELETE FROM items WHERE id=$1")
        .bind(adopted)
        .execute(&mut *conn)
        .await?;
    let after = cuts::counts(&mut conn, cut.id).await?;
    assert_eq!(after, before, "the captured adopted line is still counted");
    Ok(())
}

#[tokio::test]
async fn a_cut_holds_its_items_by_spans_and_keeps_a_copy_only_of_an_item_changed_or_removed_after_it()
-> Proof {
    let world = common::World::new().await;
    let thread = conversation(&world, 3).await?;
    let cut = captured(&world, thread).await?;
    let mut conn = world.pool.acquire().await?;
    let copies = |cut: Id| {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM thread_history_cut_items WHERE cut_id=$1",
        )
        .bind(cut)
    };
    assert_eq!(
        copies(cut.id).fetch_one(&mut *conn).await?,
        0,
        "nothing is copied when a cut is made"
    );
    let held = cuts::items_between(&mut conn, cut.id, 0, i64::MAX, None).await?;
    assert!(held.len() >= 3, "the three turns' items are held");
    // One item is changed in place and one is removed; the cut still reads both as they stood.
    let changed = &held[0];
    let removed = held
        .iter()
        .rev()
        .find(|item| item.kind == "agent_message")
        .ok_or("the cut holds an answer")?;
    sqlx::query(
        "UPDATE items SET payload=payload||'{\"text\":\"changed after the cut\"}'::jsonb,
        seq_updated=(SELECT next_seq FROM threads WHERE node_id=$2) WHERE id=$1",
    )
    .bind(changed.id)
    .bind(thread)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE threads SET next_seq=next_seq+1 WHERE node_id=$1")
        .bind(thread)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM items WHERE id=$1")
        .bind(removed.id)
        .execute(&mut *conn)
        .await?;
    assert_eq!(
        copies(cut.id).fetch_one(&mut *conn).await?,
        2,
        "one copy for each item changed or removed"
    );
    let after = cuts::items_between(&mut conn, cut.id, 0, i64::MAX, None).await?;
    assert_eq!(
        after
            .iter()
            .map(|item| (item.id, item.seq_created, item.payload.clone()))
            .collect::<Vec<_>>(),
        held.iter()
            .map(|item| (item.id, item.seq_created, item.payload.clone()))
            .collect::<Vec<_>>(),
        "the cut reads every item as it stood when the cut was made"
    );
    Ok(())
}

#[tokio::test]
async fn pages_of_turns_give_the_turns_and_items_the_whole_reading_gives_and_a_turn_without_items_too()
-> Proof {
    let world = common::World::new().await;
    let thread = conversation(&world, 7).await?;
    // A turn that made nothing is still a turn: copy the last one without its items.
    sqlx::query(
        "INSERT INTO turns SELECT (jsonb_populate_record(NULL::turns, to_jsonb(t) || jsonb_build_object(
            'id', gen_random_uuid(), 'ordinal', 8, 'native_turn_ref', NULL))).* FROM turns t WHERE t.thread_id=$1 AND t.ordinal=7",
    )
    .bind(thread)
    .execute(world.pool.pool())
    .await?;
    let mut conn = world.pool.acquire().await?;
    let row = threads::thread(&mut conn, thread).await?.ok_or("absent")?;
    let view = row.active_continuation_id;
    let whole_turns = thread_history::turns_between(&mut conn, thread, view, 1, 8).await?;
    assert_eq!(whole_turns.len(), 8);
    let mut expected = Vec::new();
    let mut previous = 0;
    let mut loose_ids = Vec::new();
    for turn in &whole_turns {
        let mut items = thread_history::items_of_turns(&mut conn, thread, view, &[turn.id]).await?;
        // The whole written reading also includes notices outside a turn. As in cut_rendered, a notice before a turn
        // or between its items belongs to that turn; items_of_turns alone cannot be the reference for that reading.
        let through = thread_history::turn_high(&mut conn, thread, view, i64::from(turn.ordinal))
            .await?
            .unwrap_or(previous);
        let loose =
            thread_history::items_between(&mut conn, thread, view, previous, through, Some("main"))
                .await?;
        for item in loose.into_iter().filter(|item| item.turn_id.is_none()) {
            loose_ids.push(item.id);
            items.push(item);
        }
        previous = through;
        items.sort_by_key(|item| item.seq_created);
        expected.push((
            turn.ordinal,
            items
                .iter()
                .map(|item| (item.id, item.kind.clone()))
                .collect::<Vec<_>>(),
        ));
    }
    assert!(
        !loose_ids.is_empty(),
        "the fixture must retain the notice before its first turn"
    );
    // Oldest first, three at a time.
    let source = Source::Live { thread, view };
    let mut reader = Forward::between(source, &mut conn, (1, 8), false).await?;
    let mut forward = Vec::new();
    loop {
        let page = reader.next(&mut conn, 3).await?;
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 3);
        forward.extend(page.into_iter().map(|bundle| {
            (
                bundle.turn.ordinal,
                bundle
                    .items
                    .iter()
                    .map(|item| (item.id, item.kind.clone()))
                    .collect::<Vec<_>>(),
            )
        }));
    }
    assert_eq!(forward, expected);
    for id in loose_ids {
        assert_eq!(
            forward
                .iter()
                .flat_map(|(_, items)| items)
                .filter(|(item, _)| *item == id)
                .count(),
            1,
            "a notice outside a turn must be retained exactly once"
        );
    }
    let reads = reader.reads();
    assert_eq!(
        reads.items,
        u64::try_from(expected.iter().map(|(_, items)| items.len()).sum::<usize>())?
    );
    assert!(
        reads.peak_held < 6 * thread_history_cuts_page(),
        "{reads:?}"
    );
    // Newest first, two at a time: the same turns in the other order, each turn's items still oldest first.
    let mut reader = Backward::between(source, &mut conn, (1, 8), 8, false).await?;
    let mut backward = Vec::new();
    loop {
        let page = reader.next(&mut conn, 2).await?;
        if page.is_empty() {
            break;
        }
        backward.extend(page.into_iter().map(|bundle| {
            (
                bundle.turn.ordinal,
                bundle
                    .items
                    .iter()
                    .map(|item| (item.id, item.kind.clone()))
                    .collect::<Vec<_>>(),
            )
        }));
    }
    backward.reverse();
    assert_eq!(backward, expected);
    // A range inside the history gives exactly its turns.
    let mut reader = Forward::between(source, &mut conn, (3, 5), false).await?;
    let middle: Vec<_> = reader
        .next(&mut conn, 100)
        .await?
        .into_iter()
        .map(|bundle| bundle.turn.ordinal)
        .collect();
    assert_eq!(middle, [3, 4, 5]);
    drop(conn);
    world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_transcript_written_in_pages_is_the_file_the_whole_reading_wrote_on_a_small_thread()
-> Proof {
    let world = common::World::new().await;
    let store = std::sync::Arc::new(
        orch_blob_local::LocalBlobStore::open(
            std::env::temp_dir().join(format!("orch-pages-{}", new_id())),
        )
        .await?,
    );
    world.engine().attach_blobs(store);
    let thread = conversation(&world, 6).await?;
    let cut = captured(&world, thread).await?;
    let mut conn = world.pool.acquire().await?;
    // The cut, through the file.
    let artifact = cut::transcript_file(&world.engine(), &cut, "transcript").await?;
    assert_eq!(artifact.turns, 6);
    let written = stored_text(&world, &artifact).await?;
    assert_eq!(
        written,
        reference::cut_transcript(&mut conn, &cut).await?,
        "the transcript of a cut"
    );
    assert_eq!(i64::try_from(written.len())?, artifact.size);
    // The selected history, through the file.
    let row = threads::thread(&mut conn, thread).await?.ok_or("absent")?;
    let file = std::env::temp_dir().join(format!("orch-live-{}.md", new_id()));
    let mut reading = orch_db::queries::feeds::begin_reading(world.engine().db()).await?;
    let made =
        crate::summaries::indexed_transcript_file(&mut reading, &row, None, None, &file).await?;
    reading.commit().await?;
    let (old, turns) = reference::live_transcript(&mut conn, &row, None, None).await?;
    assert_eq!(
        std::fs::read_to_string(&file)?,
        old,
        "the transcript of the live history"
    );
    assert_eq!(made.turns, turns);
    // A range of it, too.
    let mut reading = orch_db::queries::feeds::begin_reading(world.engine().db()).await?;
    let ranged =
        crate::summaries::indexed_transcript_file(&mut reading, &row, Some(2), Some(4), &file)
            .await?;
    reading.commit().await?;
    let (old, turns) = reference::live_transcript(&mut conn, &row, Some(2), Some(4)).await?;
    assert_eq!(
        std::fs::read_to_string(&file)?,
        old,
        "the transcript of a range"
    );
    assert_eq!(ranged.turns, turns);
    let _ = std::fs::remove_file(&file);
    drop(conn);
    world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn the_carry_takes_the_newest_turns_that_fit_as_the_whole_reading_chose_them() -> Proof {
    let world = common::World::new().await;
    let thread = conversation(&world, 9).await?;
    let cut = captured(&world, thread).await?;
    let mut conn = world.pool.acquire().await?;
    let (_, _, texts) = reference::cut_rendered(&mut conn, &cut, Detail::Normal).await?;
    let one = texts.last().map_or(0, String::len) + 1;
    for (detail, budget, last) in [
        (Detail::Normal, Some(one * 4 + 3), None),
        (Detail::Normal, Some(one * 20), None),
        (Detail::Normal, Some(5), None),
        (Detail::Brief, None, Some(3)),
        (Detail::Normal, Some(one * 4 + 3), Some(2)),
    ] {
        let old = reference::cut_recent(&mut conn, &cut, detail, budget, last).await?;
        let new = cut::recent(&mut conn, &cut, detail, budget, last).await?;
        assert_eq!(
            new.map(|carried| (carried.from, carried.to, carried.text)),
            old,
            "{detail:?} {budget:?} {last:?}"
        );
    }
    drop(conn);
    world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn the_newest_turns_are_those_whose_neutral_lines_fit_and_a_bound_that_holds_all_leaves_nothing_out()
-> Proof {
    let world = common::World::new().await;
    let thread = conversation(&world, 10).await?;
    let mut conn = world.pool.acquire().await?;
    let row = threads::thread(&mut conn, thread).await?.ok_or("absent")?;
    let view = row.active_continuation_id;
    let source = Source::Live { thread, view };
    // Everything fits.
    let all = fork_history::newest(&mut conn, source, (1, 10), 10, 10_000_000).await?;
    assert!(all.complete);
    assert_eq!(all.selected.turns.len(), 10);
    assert_eq!(all.selected.left_out_before, None);
    assert_eq!(all.bytes, fork_history::neutral_lines(&all.selected).len());
    // Half of it fits: the newest turns, and the line that says where they begin.
    let some = fork_history::newest(&mut conn, source, (1, 10), 10, all.bytes / 2).await?;
    assert!(!some.complete);
    let taken = some.selected.turns.len();
    assert!(taken > 0 && taken < 10, "{taken}");
    assert_eq!(
        some.selected.turns.last().map(|(_, ordinal)| *ordinal),
        Some(10),
        "the newest turn is the first taken"
    );
    let first = some.selected.turns[0].1;
    assert_eq!(some.selected.left_out_before, Some(first));
    assert_eq!(
        first,
        10 - i32::try_from(taken)? + 1,
        "the turns taken are the newest, side by side"
    );
    assert!(fork_history::neutral_lines(&some.selected).len() <= all.bytes / 2);
    // The bound is exact: the turns taken fit in what they came to, and a byte less leaves the oldest of them out.
    let same = fork_history::newest(&mut conn, source, (1, 10), 10, some.bytes).await?;
    assert_eq!(same.selected.turns.len(), taken);
    let less = fork_history::newest(&mut conn, source, (1, 10), 10, some.bytes - 1).await?;
    assert_eq!(less.selected.turns.len(), taken - 1);
    // A range that ends before the end of the history is read from where it ends.
    let ranged = fork_history::newest(&mut conn, source, (2, 6), 10, 10_000_000).await?;
    assert!(ranged.complete);
    assert_eq!(
        ranged
            .selected
            .turns
            .iter()
            .map(|(_, n)| *n)
            .collect::<Vec<_>>(),
        [2, 3, 4, 5, 6]
    );
    assert_eq!(ranged.selected.left_out_before, Some(2));
    drop(conn);
    world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn an_oversized_neutral_turn_stops_after_one_item_page_without_holding_the_turn() -> Proof {
    let world = common::World::new().await;
    let thread = conversation(&world, 1).await?;
    let mut conn = world.pool.acquire().await?;
    let row = threads::thread(&mut conn, thread).await?.ok_or("absent")?;
    let turn: Id = world
        .scalar(
            "SELECT id FROM turns WHERE thread_id=$1 AND ordinal=1",
            thread,
        )
        .await;
    let base: i64 = world
        .scalar("SELECT next_seq FROM threads WHERE node_id=$1", thread)
        .await;
    let end = base + SCALE_TURNS + 1;
    let mut tx = world.pool.begin().await?;
    // One complete turn can contain as many items as a long history. Keep the template conversion lateral, once
    // per generated row, and put the turn's end and next sequence after every copied item.
    sqlx::query("UPDATE turns SET completed_seq=$2,seq_updated=$2 WHERE id=$1")
        .bind(turn)
        .bind(end)
        .execute(&mut *tx)
        .await?;
    // The copies name what the template names: the reference trigger's reading of each, a row at a time, is no part
    // of what this turn proves, so it is off for this one statement.
    sqlx::query("ALTER TABLE items DISABLE TRIGGER related_thread_references")
        .execute(&mut *tx)
        .await?;
    let inserted = sqlx::query(
        "WITH seed AS MATERIALIZED (
            SELECT to_jsonb(i) AS body FROM items i
            WHERE i.turn_id=$2 AND i.kind='agent_message' AND i.stream='main'
            ORDER BY i.seq_created DESC LIMIT 1)
         INSERT INTO items SELECT copied.*
         FROM seed, generate_series(1,$3::bigint) g(n)
         CROSS JOIN LATERAL jsonb_populate_record(NULL::items, seed.body || jsonb_build_object(
            'id', md5($1::text||'giant-neutral-item'||g.n::text)::uuid,
            'native_ref', NULL, 'search_text', NULL,
            'seq_created', $4::bigint+g.n, 'seq_updated', $4::bigint+g.n)) AS copied",
    )
    .bind(thread)
    .bind(turn)
    .bind(SCALE_TURNS)
    .bind(base)
    .execute(&mut *tx)
    .await?;
    sqlx::query("ALTER TABLE items ENABLE TRIGGER related_thread_references")
        .execute(&mut *tx)
        .await?;
    assert_eq!(inserted.rows_affected(), u64::try_from(SCALE_TURNS)?);
    sqlx::query("UPDATE threads SET next_seq=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(end + 1)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    world.exec("ANALYZE items").await;
    let cut = captured(&world, thread).await?;
    // A KiB cannot hold this turn, so the whole-turn rule leaves it out. Its size must be discovered in the first
    // ordinary item page, without retaining or reading the remaining items, on either source.
    let budget = 1_024;
    for source in [
        Source::Live {
            thread,
            view: row.active_continuation_id,
        },
        Source::Cut(&cut),
    ] {
        let newest = fork_history::newest(&mut conn, source, (1, 1), 1, budget).await?;
        assert!(!newest.complete);
        assert!(newest.selected.turns.is_empty());
        assert!(newest.selected.items.is_empty());
        assert_eq!(newest.selected.left_out_before, Some(2));
        assert!(fork_history::neutral_lines(&newest.selected).len() <= budget);
        assert_eq!(newest.reads.item_pages, 1, "{:?}", newest.reads);
        assert!(newest.reads.items <= u64::try_from(cuts::PAGE)?);
        assert!(
            newest.reads.peak_held <= usize::try_from(cuts::PAGE + TURN_PAGE + 1)?,
            "{:?}",
            newest.reads
        );
    }
    drop(conn);
    world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_history_of_two_hundred_thousand_turns_is_counted_in_sql_and_its_carry_reads_only_what_fits()
-> Proof {
    let world = common::World::new().await;
    let thread = conversation(&world, 1).await?;
    world.grow_history(thread, SCALE_TURNS).await;
    let mut conn = world.pool.acquire().await?;
    let row = threads::thread(&mut conn, thread).await?.ok_or("absent")?;
    let view = row.active_continuation_id;
    // The plan's counting: a few statements, whatever the length.
    let (counted, counting) = Statements::of(thread_history::range_counts(
        &mut conn,
        thread,
        view,
        1,
        SCALE_TURNS,
    ))
    .await;
    let counted = counted?;
    assert_eq!(counted.turns, SCALE_TURNS);
    assert_eq!(counted.items["user_message"], SCALE_TURNS);
    assert_eq!(counted.items["agent_message"], SCALE_TURNS);
    assert!(counting <= 4, "{counting} statements counted the history");
    // The carry's reading: the newest turns that fit, a page at a time, stopping there.
    let source = Source::Live { thread, view };
    let budget = 64 * 1024;
    let (newest, reading) = Statements::of(fork_history::newest(
        &mut conn,
        source,
        (1, SCALE_TURNS),
        SCALE_TURNS,
        budget,
    ))
    .await;
    let newest = newest?;
    assert!(!newest.complete);
    let taken = i64::try_from(newest.selected.turns.len())?;
    assert!(taken > 10 && taken < 5_000, "{taken} turns fit 64 KiB");
    assert_eq!(
        newest
            .selected
            .turns
            .last()
            .map(|(_, ordinal)| i64::from(*ordinal)),
        Some(SCALE_TURNS)
    );
    // What was read is the turns that fit, one more page, and no more: never the history.
    let reads = newest.reads;
    assert!(
        reads.items <= u64::try_from(taken * 2)? + 2 * u64::try_from(thread_history_cuts_page())?,
        "{reads:?}"
    );
    assert!(
        reads.turns <= 2 * u64::try_from(thread_history_cuts_page())? + 200,
        "{reads:?}"
    );
    assert!(reading <= 20, "{reading} statements read {taken} turns");
    assert!(
        reads.peak_held < 6 * thread_history_cuts_page(),
        "{reads:?}"
    );
    // The same figures for a thread of three turns, to see that the length of the history is not in them.
    let small = conversation(&world, 3).await?;
    let small_row = threads::thread(&mut conn, small).await?.ok_or("absent")?;
    let (small_newest, small_reading) = Statements::of(fork_history::newest(
        &mut conn,
        Source::Live {
            thread: small,
            view: small_row.active_continuation_id,
        },
        (1, 3),
        3,
        budget,
    ))
    .await;
    small_newest?;
    assert!(
        reading <= small_reading + 12,
        "{small_reading} statements for three turns, {reading} for the large history"
    );
    drop(conn);
    world.shutdown().await;
    Ok(())
}

fn thread_history_cuts_page() -> usize {
    usize::try_from(cuts::PAGE).unwrap_or(500)
}

#[tokio::test]
async fn a_summary_of_a_large_cut_reads_only_the_brief_tail_and_newest_older_index_lines() -> Proof
{
    let world = common::World::new().await;
    let thread = conversation(&world, 1).await?;
    // Half of a 4,000-token summary window, using the carry rule's three bytes to a token.
    let budget = crate::summaries::InputBudget {
        window: Some(4_000),
        share: 50,
        tokens: 2_000,
        bytes: 6_000,
    };
    let small = captured(&world, thread).await?;
    let (short, small_queries) =
        Statements::of(cut::summary_input(&world.engine(), &small, &budget)).await;
    let short = short?;
    assert!(short.starts_with("# Newest turns, in brief"));
    assert!(short.contains("answer 0"));
    assert!(!short.contains("still older are not listed"));
    // A second thread lets its immutable cut have its own identity.
    let large = conversation(&world, 1).await?;
    world.grow_history(large, SCALE_TURNS).await;
    let large = captured(&world, large).await?;
    let (sent, queries) =
        Statements::of(cut::summary_input(&world.engine(), &large, &budget)).await;
    let sent = sent?;
    assert!(sent.len() <= budget.bytes - crate::summaries::HANDOFF_FRAME_BYTES);
    assert!(sent.starts_with("# Newest turns, in brief"));
    assert!(sent.contains("# Older turns, one line each"));
    assert!(sent.contains("still older are not listed"));
    assert!(!sent.contains("- Turn 1 ·"));
    assert!(!sent.contains(" · line "));
    let indexed: Vec<i64> = sent
        .lines()
        .filter_map(|line| {
            line.strip_prefix("- Turn ")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .collect();
    assert!(!indexed.is_empty());
    assert!(
        indexed.windows(2).all(|pair| pair[0] > pair[1]),
        "{indexed:?}"
    );
    assert!(indexed[0] > SCALE_TURNS - 100, "{indexed:?}");
    // At this budget a single 25-turn render page is enough; allow a second page's batched metadata queries.
    assert!(
        queries <= small_queries + 12,
        "{small_queries} queries for one turn; {queries} for {SCALE_TURNS}"
    );
    world.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_cut_of_two_hundred_thousand_turns_gives_its_carry_from_the_end_its_transcript_and_its_copy_a_page_at_a_time()
-> Proof {
    let world = common::World::new().await;
    let store = std::sync::Arc::new(
        orch_blob_local::LocalBlobStore::open(
            std::env::temp_dir().join(format!("orch-pages-{}", new_id())),
        )
        .await?,
    );
    world.engine().attach_blobs(store);
    let thread = conversation(&world, 1).await?;
    world.grow_history(thread, SCALE_TURNS).await;
    let cut = captured(&world, thread).await?;
    let mut conn = world.pool.acquire().await?;
    let counted = cuts::counts(&mut conn, cut.id).await?;
    assert_eq!(counted.turns, SCALE_TURNS);
    // The carry: the newest turns within the budget, read from the end of the cut.
    let (carried, reading) = Statements::of(cut::recent(
        &mut conn,
        &cut,
        Detail::Normal,
        Some(64 * 1024),
        None,
    ))
    .await;
    let carried = carried?.ok_or("nothing was carried")?;
    assert_eq!(carried.to, SCALE_TURNS);
    assert!(carried.text.len() <= 64 * 1024 + 80);
    assert!(
        carried.to - carried.from < 5_000,
        "turns {} to {} fit 64 KiB",
        carried.from,
        carried.to
    );
    assert!(
        reading < 120,
        "{reading} statements read the end of the cut"
    );
    drop(conn);
    // The transcript: every turn, a page at a time. What is held is a page, and what is written is the file.
    let artifact = cut::transcript_file(&world.engine(), &cut, "transcript").await?;
    assert_eq!(artifact.turns, SCALE_TURNS);
    let store = world.engine().blobs().ok_or("no store")?;
    let mut body = store.get(&artifact.blob, None).await?.body;
    let (mut lines, mut indexed, mut bytes) = (0_u64, 0_u64, 0_i64);
    let mut partial = String::new();
    while let Some(piece) = futures::StreamExt::next(&mut body).await {
        let piece = piece?;
        bytes += i64::try_from(piece.len())?;
        partial.push_str(&String::from_utf8_lossy(&piece));
        while let Some(end) = partial.find('\n') {
            let line: String = partial.drain(..=end).collect();
            lines += 1;
            if line.starts_with("- Turn ") {
                indexed += 1;
            }
        }
    }
    assert_eq!(bytes, artifact.size);
    assert_eq!(
        i64::try_from(indexed)?,
        SCALE_TURNS,
        "a line of the index for each turn"
    );
    assert!(lines > u64::try_from(SCALE_TURNS)?);
    // The conversation is copied into a thread of its own: the same statements for every page of items, with no table of
    // the identities of all of them held or sent. (Such a table of 400,000 items was sent again with each of 800 pages.)
    let target = world.thread("A copy of a long history").await;
    let target_segment = world.segment_of(target).await;
    let mut tx = world.pool.begin().await?;
    // A fork publishes through the API pool, whose statement budget is 15 seconds.
    sqlx::query("SET LOCAL statement_timeout='15s'")
        .execute(&mut *tx)
        .await?;
    let (copied, statements) = Statements::of(async {
        let mut unit = crate::unit::Unit::new(&mut tx, world.clock.now());
        fork_history::copy_conversation(
            &mut unit,
            thread,
            target,
            target_segment,
            Some((1, i32::try_from(SCALE_TURNS)?)),
            None,
        )
        .await?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await;
    copied?;
    tx.commit().await?;
    let pages = 2 * SCALE_TURNS / cuts::PAGE;
    assert!(
        i64::try_from(statements)? <= 4 * pages + 200,
        "{statements} statements copied {pages} pages of items"
    );
    let copy: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM turns WHERE thread_id=$1),(SELECT count(*) FROM items WHERE thread_id=$1)",
    )
    .bind(target)
    .fetch_one(world.pool.pool())
    .await?;
    assert_eq!(copy, (SCALE_TURNS, 2 * SCALE_TURNS));
    // An identity in the copy is made from the target's and the original's.
    let (made, derived): (Id, Id) = sqlx::query_as(
        "SELECT t.id, md5($1::text||o.id::text)::uuid FROM turns t, turns o
          WHERE t.thread_id=$1 AND t.ordinal=777 AND o.thread_id=$2 AND o.ordinal=777",
    )
    .bind(target)
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await?;
    assert_eq!(made, derived);
    world.shutdown().await;
    Ok(())
}

/// Six thousand empty turns exceed the former 5000-row lookahead. Both directions must retain the distant
/// item while returning just the requested empty turns, from the live view and from an immutable cut.
#[tokio::test]
async fn sparse_turns_keep_their_items_without_looking_ahead_through_the_empty_history() -> Proof {
    const TURNS: i64 = 6_002;
    const ASKED: usize = 25;
    let world = common::World::new().await;
    let thread = conversation(&world, 1).await?;
    world.grow_history(thread, TURNS).await;
    sqlx::query(
        "DELETE FROM items i USING turns t WHERE i.turn_id=t.id AND t.thread_id=$1
          AND t.ordinal>1 AND t.ordinal<$2",
    )
    .bind(thread)
    .bind(TURNS)
    .execute(world.pool.pool())
    .await?;
    let expected: Vec<(i32, Id)> = sqlx::query_as(
        "SELECT t.ordinal,i.id FROM items i JOIN turns t ON t.id=i.turn_id
          WHERE t.thread_id=$1 AND i.kind='agent_message' ORDER BY t.ordinal",
    )
    .bind(thread)
    .fetch_all(world.pool.pool())
    .await?;
    assert_eq!(
        expected.len(),
        2,
        "only the first and last turns have answers"
    );
    assert_eq!(expected[0].0, 1);
    assert_eq!(i64::from(expected[1].0), TURNS);
    let cut = captured(&world, thread).await?;
    let mut conn = world.pool.acquire().await?;
    let view = threads::thread(&mut conn, thread)
        .await?
        .ok_or("absent")?
        .active_continuation_id;
    fn check(bundle: Bundle, ordinal: i64, expected: &[(i32, Id)]) {
        assert_eq!(i64::from(bundle.turn.ordinal), ordinal);
        let answers: Vec<Id> = bundle
            .items
            .iter()
            .filter(|item| item.kind == "agent_message")
            .map(|item| item.id)
            .collect();
        let wanted: Vec<Id> = expected
            .iter()
            .filter(|(turn, _)| i64::from(*turn) == ordinal)
            .map(|(_, id)| *id)
            .collect();
        assert_eq!(answers, wanted, "answer membership of turn {ordinal}");
    }
    for source in [Source::Live { thread, view }, Source::Cut(&cut)] {
        let mut forward = Forward::between(source, &mut conn, (1, TURNS), false).await?;
        let mut page = forward.next(&mut conn, ASKED).await?;
        assert_eq!(page.len(), ASKED);
        assert!(
            forward.reads().turns <= u64::try_from(TURN_PAGE)?,
            "{:?}",
            forward.reads()
        );
        let mut ordinal = 1;
        loop {
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= ASKED);
            for bundle in page {
                check(bundle, ordinal, &expected);
                ordinal += 1;
            }
            page = forward.next(&mut conn, ASKED).await?;
        }
        assert_eq!(ordinal, TURNS + 1);
        assert!(forward.reads().peak_held <= 1_000, "{:?}", forward.reads());
        let mut backward = Backward::between(source, &mut conn, (1, TURNS), TURNS, false).await?;
        let mut page = backward.next(&mut conn, ASKED).await?;
        assert_eq!(page.len(), ASKED);
        assert!(
            backward.reads().turns <= u64::try_from(TURN_PAGE)?,
            "{:?}",
            backward.reads()
        );
        let mut ordinal = TURNS;
        loop {
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= ASKED);
            for bundle in page {
                check(bundle, ordinal, &expected);
                ordinal -= 1;
            }
            page = backward.next(&mut conn, ASKED).await?;
        }
        assert_eq!(ordinal, 0);
        assert!(
            backward.reads().peak_held <= 1_000,
            "{:?}",
            backward.reads()
        );
    }
    drop(conn);
    world.shutdown().await;
    Ok(())
}
