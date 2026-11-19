//! A genuine bounded summary beside the indexed full transcript a new agent can verify.
//!
//! The model is not sent the whole conversation: it is sent the newest turns in brief and one line for each older
//! turn, newest first, as much as the place's share of the model's window allows (`summary.handoff_input_share`).
//! The full transcript stays what it was, a file the new agent reads.

use super::budget::InputBudget;
use super::{choice, internal, measure};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::written::{self, Detail};
use orch_db::queries::{feeds, thread_history, thread_history_cuts, threads};
use orch_plugin_contract::refs::{NodeKind, NodeRef};
use orch_summarizer_contract::descriptor::SummaryField;
use orch_summarizer_contract::summarizer::{SummaryLimits, SummaryRequest};
use orch_types::{Id, Json};
use serde_json::json;

/// The bytes the instruction and the frame around the material take, which the share leaves room for.
pub(crate) const FRAME_BYTES: usize = 1_200;

/// How much of the room the newest turns in brief may take; the rest is for the index of older turns.
const BRIEF_SHARE_PERCENT: usize = 75;

/// The bytes the two headings of the material take, which its lines leave room for.
pub(crate) const HEADINGS_BYTES: usize = 200;

/// How many turns `brief_tail` reads at a time while it looks for the newest ones that fit.
const TAIL_WINDOW_TURNS: i64 = 256;

#[cfg(test)]
mod tests;

/// The full file and the number of turns its index names.
pub struct IndexedTranscript {
    /// The complete written conversation, with the index before it.
    pub text: String,
    /// The number of full turns in the file.
    pub turns: i64,
}

/// Makes the indexed full file without asking a model or calling it a summary.
pub async fn indexed_transcript(
    engine: &Engine,
    thread: Id,
    through: Option<i64>,
) -> Result<IndexedTranscript, EngineError> {
    indexed_transcript_range(engine, thread, None, through).await
}

/// The selected whole turns of a confirmed range, indexed without including unselected earlier work.
pub(crate) async fn indexed_transcript_range(
    engine: &Engine,
    thread: Id,
    from: Option<i64>,
    through: Option<i64>,
) -> Result<IndexedTranscript, EngineError> {
    // The ordinary feed snapshot supplies one metadata cut. This infrequent full
    // handoff performs only SQL/rendering inside it, never provider/blob/machine I/O.
    let mut conn = feeds::begin_reading(engine.db()).await?;
    let row = threads::thread(&mut conn, thread)
        .await?
        .ok_or_else(|| EngineError::unreadable("the source conversation is unavailable"))?;
    #[cfg(test)]
    tests::captured(thread, row.active_continuation_id).await;
    let transcript = indexed_transcript_in(&mut conn, &row, from, through).await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(transcript)
}

/// How many turns are written at a time: enough that the queries of a page are shared by many turns, few enough that a
/// page of written turns is held without notice.
const TURNS_PER_PAGE: usize = 25;

/// The most a transcript may be that is held as a string: 4 MiB. The transcript of a long conversation is a file, written
/// a page at a time and never held ([`indexed_transcript_file`]); a string is for the summary of a short one.
pub const HELD_TRANSCRIPT_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// A transcript written to a file: how many turns its index names and how large it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrittenTranscript {
    /// The turns in its index.
    pub turns: i64,
    /// Its size in bytes.
    pub size: i64,
}

/// What a page of turns is written as: each turn, its index line and its text.
struct LivePage {
    preview: String,
    text: String,
}

/// Writes a page of turns of the selected history as the written form of a conversation gives them: one turn to a
/// reading, under the head of the thread.
async fn render_page(
    conn: &mut orch_db::Conn,
    thread: Id,
    view: Id,
    head: &written::Head<'_>,
    last: i64,
    bundles: Vec<crate::history_pages::Bundle>,
) -> Result<Vec<LivePage>, EngineError> {
    use crate::views::{People, item_view, turn_view};
    use orch_api_contract::threads::{ItemPayload, ToolPayload};
    use orch_db::queries::shares as rows;
    let mut who: Vec<Id> = Vec::new();
    for bundle in &bundles {
        who.extend(bundle.items.iter().filter_map(|item| item.author_id));
        who.extend(bundle.turn.interrupted_by);
    }
    let people = People::load(conn, who.iter().copied()).await?;
    let names: std::collections::HashMap<Id, String> = threads::people(conn, &who)
        .await?
        .into_iter()
        .map(|person| (person.id, person.display_name))
        .collect();
    let mut typed = Vec::with_capacity(bundles.len());
    for bundle in &bundles {
        typed.push((
            turn_view(&bundle.turn, "", &people)?,
            bundle
                .items
                .iter()
                .map(|item| item_view(item, &people))
                .collect::<Vec<_>>(),
        ));
    }
    let mut approvals: Vec<Id> = Vec::new();
    let mut streams: Vec<String> = Vec::new();
    for (_, items) in &typed {
        for item in items {
            match &item.payload {
                ItemPayload::Approval(approval) => approvals.push(approval.approval_id),
                ItemPayload::ToolCall(call) => {
                    if let ToolPayload::Subagent { child_stream, .. } = &call.tool {
                        streams.push(child_stream.clone());
                    }
                }
                _ => {}
            }
        }
    }
    let mut beside = written::Beside::default();
    if !approvals.is_empty() {
        for line in rows::approval_lines(conn, &approvals).await? {
            beside.approvals.insert(
                line.id,
                written::ApprovalFacts {
                    title: line.title,
                    decided_by: line.decided_by,
                },
            );
        }
    }
    if !streams.is_empty() {
        beside.sub_agents = thread_history::last_sub_messages(conn, thread, view, &streams).await?;
    }
    let segments: Vec<Id> = bundles
        .iter()
        .map(|bundle| bundle.turn.segment_id)
        .collect();
    let agents = rows::segment_agents(conn, &segments).await?;
    let mut out = Vec::with_capacity(bundles.len());
    for (bundle, (turn, items)) in bundles.iter().zip(&typed) {
        let part = written::Part {
            turn,
            agent: agents
                .get(&bundle.turn.segment_id)
                .map_or(head.agent, String::as_str),
            items: items.iter().collect(),
        };
        let text = written::turn_text(&part, &beside, Detail::Full);
        let first = bundle
            .items
            .iter()
            .find(|item| item.kind == "user_message" && item.turn_id == Some(bundle.turn.id));
        let author = first
            .and_then(|item| item.author_id)
            .and_then(|id| names.get(&id))
            .map(String::as_str);
        let asked = first
            .map(|item| &item.payload)
            .and_then(|payload| payload["parts"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|part| part.get("text").and_then(Json::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        out.push(LivePage {
            preview: crate::fork_history::cut::preview_of(&bundle.turn, author, &asked),
            text: written::conversation(
                head,
                i64::from(bundle.turn.ordinal),
                i64::from(bundle.turn.ordinal),
                last,
                &[text],
            ),
        });
    }
    Ok(out)
}

/// Writes the indexed full transcript of the selected history of `row` to the file `destination`, a page of turns at a
/// time: an index with a line for each turn, then each turn written in full under the head of the thread, redacted as the
/// source is. Every page is read through `conn`, which the caller holds on one snapshot of the history; the turns are
/// appended to a file as they are written and the lines of the index are spooled to a second, since each names the
/// line its turn begins at, and the two are joined at the end. Only the page being written is held, whatever the length
/// of the conversation.
pub(crate) async fn indexed_transcript_file(
    conn: &mut orch_db::Conn,
    row: &threads::ThreadRow,
    from: Option<i64>,
    through: Option<i64>,
    destination: &std::path::Path,
) -> Result<WrittenTranscript, EngineError> {
    use std::io::Write as _;
    let thread = row.node_id;
    let view = row.active_continuation_id;
    let last = thread_history::last_turn(conn, thread, view).await?;
    let low = from.unwrap_or(1).max(1);
    let mut high = through.unwrap_or(last).min(last);
    if through.is_none() {
        // Only the turns up to the last one that completed are written.
        let completed = thread_history_cuts::last_completed(conn, thread, view)
            .await?
            .map_or(0, i64::from);
        high = high.min(completed);
    }
    let redactor = crate::raw_events::live_redactor(conn, row, row.environment_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the source conversation cannot be redacted"))?;
    let agent_name = match row.agent_id {
        Some(agent) => orch_db::queries::shares::agent_names(conn, &[agent])
            .await?
            .remove(&agent)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Agent".to_owned()),
        None => "Agent".to_owned(),
    };
    let head = written::Head {
        name: &row.name,
        agent: &agent_name,
        state: &row.state,
    };
    let scratch = |extension: &str| {
        crate::fork_history::cut::Scratch::new(destination.with_extension(extension))
    };
    let (body, spool) = (scratch("body"), scratch("index"));
    let files = |error: std::io::Error| {
        EngineError::unreadable(format!("the transcript could not be written: {error}"))
    };
    let mut body_file = std::io::BufWriter::new(std::fs::File::create(body.path()).map_err(files)?);
    let mut index_file =
        std::io::BufWriter::new(std::fs::File::create(spool.path()).map_err(files)?);
    let mut turns = 0_i64;
    let mut offset = 0_usize;
    if low <= high {
        let mut reader = crate::history_pages::Forward::between(
            crate::history_pages::Source::Live { thread, view },
            conn,
            (low, high),
            true,
        )
        .await?;
        loop {
            let bundles = reader.next(conn, TURNS_PER_PAGE).await?;
            if bundles.is_empty() {
                break;
            }
            for each in render_page(conn, thread, view, &head, last, bundles).await? {
                let text = crate::raw_events::mask_text(&redactor, each.text);
                let preview = crate::raw_events::mask_text(&redactor, each.preview);
                if turns > 0 {
                    body_file.write_all(b"\n").map_err(files)?;
                }
                body_file.write_all(text.as_bytes()).map_err(files)?;
                writeln!(
                    index_file,
                    "{}",
                    serde_json::to_string(&(offset, preview))
                        .map_err(|error| EngineError::unreadable(error.to_string()))?
                )
                .map_err(files)?;
                offset += text.lines().count() + 1;
                turns += 1;
            }
        }
    }
    body_file.flush().map_err(files)?;
    index_file.flush().map_err(files)?;
    drop((body_file, index_file));
    crate::fork_history::cut::join_transcript(spool.path(), body.path(), destination, turns)?;
    let size = std::fs::metadata(destination).map_err(files)?.len();
    Ok(WrittenTranscript {
        turns,
        size: i64::try_from(size).unwrap_or(i64::MAX),
    })
}

/// Reuses the indexed renderer inside a caller's already locked history transaction, for a transcript that is held as a
/// string: it is written as the file is, and read back, and one longer than [`HELD_TRANSCRIPT_MAX_BYTES`] is refused,
/// since a conversation of that length is a file and not a string.
pub(crate) async fn indexed_transcript_in(
    conn: &mut orch_db::Conn,
    row: &threads::ThreadRow,
    from: Option<i64>,
    through: Option<i64>,
) -> Result<IndexedTranscript, EngineError> {
    let path = std::env::temp_dir().join(format!(
        "orch-transcript-{}.md",
        orch_types::new_id().simple()
    ));
    let file = crate::fork_history::cut::Scratch::new(path);
    let written = indexed_transcript_file(conn, row, from, through, file.path()).await?;
    if u64::try_from(written.size).unwrap_or(u64::MAX) > HELD_TRANSCRIPT_MAX_BYTES {
        return Err(EngineError::unreadable(
            "the conversation is too long to be held as a string; it is written to a file",
        ));
    }
    let text = std::fs::read_to_string(file.path())
        .map_err(|error| EngineError::unreadable(error.to_string()))?;
    Ok(IndexedTranscript {
        text,
        turns: written.turns,
    })
}

/// What the summary model is sent of a conversation, and how it was cut to fit the share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundedInput {
    /// The text: the newest turns in brief, then the index of older turns, both newest first.
    pub text: String,
    /// How many of the newest turns are in it in brief.
    pub brief_turns: i64,
    /// How many older turns it lists by one line.
    pub indexed_turns: i64,
    /// How many older turns it does not list at all, for want of room.
    pub left_out_turns: i64,
}

/// The lines of the index at the head of an indexed transcript, each with its turn's number and without the pointer
/// into the file that only a reader of the file can follow.
fn index_lines(text: &str) -> Vec<(i64, String)> {
    text.lines()
        .skip_while(|line| !line.starts_with("- Turn "))
        .take_while(|line| line.starts_with("- Turn "))
        .filter_map(|line| {
            let number = line
                .strip_prefix("- Turn ")?
                .split(|character: char| !character.is_ascii_digit())
                .next()?
                .parse()
                .ok()?;
            let short = line.rsplit_once(" · line ").map_or(line, |(head, _)| head);
            Some((number, short.to_owned()))
        })
        .collect()
}

/// The bytes the newest turns in brief may take of what the share allows.
pub(crate) fn brief_bytes(budget: &InputBudget) -> usize {
    budget.bytes.saturating_sub(FRAME_BYTES) * BRIEF_SHARE_PERCENT / 100
}

/// `text` as it is, or its end where it is longer than `room` bytes, under a line that says the start is left out: what a
/// producer is sent never passes the share of its model's window, whoever made the material.
pub(crate) fn within(text: &str, room: usize) -> String {
    if text.len() <= room {
        return text.to_owned();
    }
    const NOTICE: &str = "(the start is left out)\n";
    let Some(room) = room.checked_sub(NOTICE.len()) else {
        return String::new();
    };
    let mut start = text.len() - room;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{NOTICE}{}", &text[start..])
}

/// Fits the material as the model producer escapes it, keeping complete UTF-8 characters and counting its notice.
/// The surrounding instruction and tags have already been admitted separately.
fn model_material_within(text: &str, room: usize) -> Option<String> {
    const NOTICE: &str = "(the start is left out)\n";
    let cost = |character: char| match character {
        '&' => 5,
        '<' | '>' => 4,
        _ => character.len_utf8(),
    };
    if text.chars().map(cost).sum::<usize>() <= room {
        return Some(text.to_owned());
    }
    let mut left = room.checked_sub(NOTICE.len())?;
    let mut start = text.len();
    for (at, character) in text.char_indices().rev() {
        let bytes = cost(character);
        if bytes > left {
            break;
        }
        left -= bytes;
        start = at;
    }
    Some(format!("{NOTICE}{}", &text[start..]))
}

/// Cuts a conversation to what the model is sent: the newest turns in brief as they were taken, and the index lines of
/// the older ones from the newest back, all of it within the budget.
pub(crate) fn bounded_input(
    transcript: &str,
    recent: Option<&Carried>,
    budget: &InputBudget,
) -> BoundedInput {
    let room = budget.bytes.saturating_sub(FRAME_BYTES);
    let brief = recent.map_or("", |carried| carried.text.as_str());
    let first_brief = recent.map_or(i64::MAX, |carried| carried.from);
    let older: Vec<(i64, String)> = index_lines(transcript)
        .into_iter()
        .filter(|(turn, _)| *turn < first_brief)
        .collect();
    let mut left = room.saturating_sub(brief.len() + HEADINGS_BYTES);
    let mut kept: Vec<&str> = Vec::new();
    for (_, line) in older.iter().rev() {
        if line.len() + 1 > left {
            break;
        }
        left -= line.len() + 1;
        kept.push(line);
    }
    let left_out = i64::try_from(older.len() - kept.len()).unwrap_or(i64::MAX);
    let mut text = String::new();
    if let Some(carried) = recent {
        text.push_str(&format!(
            "# Newest turns, in brief (turns {} to {})\n{}\n\n",
            carried.from, carried.to, carried.text
        ));
    }
    if !kept.is_empty() || left_out > 0 {
        text.push_str("# Older turns, one line each");
        if left_out > 0 {
            text.push_str(&format!(" ({left_out} still older are not listed)"));
        }
        text.push('\n');
        text.push_str(&kept.join("\n"));
        text.push_str("\n\n");
    }
    // What the share allows is a bound, not a hope: whatever was taken in is cut to it.
    let text = within(&text, room);
    BoundedInput {
        text,
        brief_turns: recent.map_or(0, |carried| carried.to - carried.from + 1),
        indexed_turns: i64::try_from(kept.len()).unwrap_or(i64::MAX),
        left_out_turns: left_out,
    }
}

/// The newest turns of a thread in brief, from the end back, as many as fit `budget` bytes: what a direct request for
/// a summary is sent beside the index. A last turn that alone is larger than the budget is taken cut.
async fn brief_tail(
    conn: &mut orch_db::Conn,
    row: &threads::ThreadRow,
    budget: usize,
) -> Result<Option<Carried>, EngineError> {
    let thread = row.node_id;
    let view = row.active_continuation_id;
    let redactor = crate::raw_events::live_redactor(conn, row, row.environment_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the source conversation cannot be redacted"))?;
    let mut taken: Vec<(i64, String)> = Vec::new();
    let mut used = 0usize;
    // A turn whose messages take more than the place lets Orch read (`thread.read_turn_bytes`), known from their
    // stored lengths, is named and left out unread.
    let readable = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Node(thread),
        orch_settings_contract::registry::keys::THREAD_READ_TURN_BYTES,
    )
    .await?
    .unwrap_or(i64::MAX);
    // The turns are read a window at a time from the end back, and only as far as the budget reaches.
    let mut high = thread_history::last_turn(conn, thread, view).await?;
    'windows: while high >= 1 {
        let low = (high - TAIL_WINDOW_TURNS + 1).max(1);
        let turns = thread_history::turns_between(conn, thread, view, low, high).await?;
        for turn in turns.iter().rev().filter(|turn| turn.state == "completed") {
            let ordinal = i64::from(turn.ordinal);
            let written: i64 = sqlx::query_scalar(
                "SELECT COALESCE(sum(payload_bytes), 0)::bigint FROM items WHERE turn_id = $1",
            )
            .bind(turn.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
            let text = if written > readable {
                format!("(Turn {ordinal}: its messages are too large to read here.)")
            } else {
                written::read_conversation_view(
                    conn,
                    thread,
                    view,
                    (Some(ordinal), Some(ordinal)),
                    Detail::Brief,
                    None,
                    None,
                )
                .await?
                .ok_or_else(|| EngineError::unreadable("a recorded source turn is unavailable"))?
                .text
            };
            // A cut inside a secret or a pattern match must not make the remaining fragment unrecognizable.
            let text = crate::raw_events::mask_text(&redactor, text);
            if used + text.len() + 1 > budget {
                if taken.is_empty() {
                    let mut end = budget.min(text.len());
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    taken.push((ordinal, text[..end].to_owned()));
                }
                break 'windows;
            }
            used += text.len() + 1;
            taken.push((ordinal, text));
        }
        high = low - 1;
    }
    let (Some((to, _)), Some((from, _))) = (taken.first(), taken.last()) else {
        return Ok(None);
    };
    let (from, to) = (*from, *to);
    let joined = taken
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let masked =
        crate::raw_events::mask_live(conn, row, row.environment_id, json!({ "text": joined }))
            .await?
            .ok_or_else(|| EngineError::unreadable("the source conversation cannot be redacted"))?;
    Ok(Some(Carried {
        from,
        to,
        brief: true,
        text: masked["text"].as_str().unwrap_or_default().to_owned(),
    }))
}

/// The share of the summary model's window that a request may send, read from the frozen choice of the producer.
pub(crate) async fn handoff_budget(
    engine: &Engine,
    thread: Id,
    frozen: &Json,
) -> Result<InputBudget, EngineError> {
    let HandoffChoice { chosen, .. } = serde_json::from_value(frozen.clone())?;
    let mut conn = engine.db().acquire().await?;
    match orch_summarizer_model::descriptor::Config::read(&chosen.config) {
        Ok(config) if chosen.kind == "model" => {
            super::budget::input_budget(&mut conn, thread, config.provider_id, &config.model_name)
                .await
        }
        _ => Ok(super::budget::budget_of(None, super::budget::DEFAULT_SHARE)),
    }
}

/// The producer interface used by a confirmed fork or switch; the request id survives a job retry.
pub async fn handoff(
    engine: &Engine,
    thread: Id,
    through: Option<i64>,
    request_id: Id,
) -> Result<String, EngineError> {
    let transcript = indexed_transcript(engine, thread, through).await?;
    handoff_transcript(engine, thread, &transcript.text, request_id).await
}

/// A request that is not a confirmed job summarizes the thread as it stands, from its own newest turns.
pub(crate) async fn handoff_transcript(
    engine: &Engine,
    thread: Id,
    previous: &str,
    request_id: Id,
) -> Result<String, EngineError> {
    let mut conn = engine.db().acquire().await?;
    let row = threads::thread(&mut conn, thread)
        .await?
        .ok_or_else(|| EngineError::unreadable("the source conversation is unavailable"))?;
    let name = crate::raw_events::mask_live(&mut conn, &row, row.environment_id, json!(row.name))
        .await?
        .ok_or_else(|| EngineError::unreadable("the source name cannot be redacted"))?;
    let frozen = capture_handoff_choice(&mut conn, thread, name.as_str()).await?;
    drop(conn);
    let budget = handoff_budget(engine, thread, &frozen).await?;
    let mut conn = engine.db().acquire().await?;
    let recent = brief_tail(&mut conn, &row, brief_bytes(&budget)).await?;
    drop(conn);
    // The material is the newest turns in brief and a line for each older turn, within the share: never the whole.
    let bounded = bounded_input(previous, recent.as_ref(), &budget);
    handoff_captured(engine, thread, &bounded.text, request_id, &frozen).await
}

#[derive(serde::Serialize, serde::Deserialize)]
struct HandoffChoice {
    chosen: super::Choice,
    name: Option<String>,
}

/// The small producer/config/name snapshot is retained before a confirmed job
/// asks for a summary. Its possibly large transcript stays in the pinned file.
pub(crate) async fn capture_handoff_choice(
    conn: &mut orch_db::Conn,
    thread: Id,
    name: Option<&str>,
) -> Result<Json, EngineError> {
    let chosen = choice(conn, thread)
        .await?
        .ok_or_else(|| EngineError::unreadable("no summary producer is configured"))?;
    Ok(serde_json::to_value(HandoffChoice {
        chosen,
        name: name.map(str::to_owned),
    })?)
}

pub(crate) async fn handoff_captured(
    engine: &Engine,
    thread: Id,
    previous: &str,
    request_id: Id,
    frozen: &Json,
) -> Result<String, EngineError> {
    let HandoffChoice { chosen, name } = serde_json::from_value(frozen.clone())?;
    let budget = handoff_budget(engine, thread, frozen).await?;
    let limits = SummaryLimits::for_field(SummaryField::Handoff, 0);
    let mut request = SummaryRequest {
        field: SummaryField::Handoff,
        node: NodeRef {
            id: thread,
            kind: NodeKind::Thread,
            name,
            redacted: false,
        },
        previous: within(previous, budget.bytes.saturating_sub(FRAME_BYTES)),
        activity: Vec::new(),
        tag_choices: Vec::new(),
        limits,
        config: chosen.config.clone(),
    };
    if chosen.kind == "model" {
        let config = orch_summarizer_model::descriptor::Config::read(&chosen.config)
            .map_err(EngineError::unreadable)?;
        let previous = std::mem::take(&mut request.previous);
        let (system, frame) = orch_summarizer_model::prompts::render(&request, &config);
        let room = budget
            .bytes
            .checked_sub(system.len().saturating_add(frame.len()))
            .ok_or_else(|| {
                EngineError::unreadable(
                    "the summary model's input allowance cannot hold its instructions",
                )
            })?;
        request.previous = model_material_within(&previous, room).ok_or_else(|| {
            EngineError::unreadable("the summary model's input allowance cannot hold the handoff material and its omission notice")
        })?;
    }
    let digest = orch_types::Hash::of(&serde_json::to_vec(
        &json!({"request":request,"writer":chosen.writer}),
    )?);
    let call = crate::ids::derived(
        "summary.handoff",
        &[
            request_id.as_bytes().as_slice(),
            digest.as_bytes().as_slice(),
        ],
    );
    let producer = engine
        .producer(&chosen, call, request_id, SummaryField::Handoff)
        .filter(|producer| producer.describe().writes(SummaryField::Handoff))
        .ok_or_else(|| {
            EngineError::unreadable("the chosen producer cannot write a handoff summary")
        })?;
    let result = producer.summarize(request).await;
    let answer = match result {
        Ok(answer) => answer.conform(SummaryField::Handoff, &limits, &[]),
        Err(error) => {
            if chosen.kind == "model" {
                engine
                    .model_output_failed(&chosen, thread, call, request_id, "handoff")
                    .await?;
            }
            measure(SummaryField::Handoff, "failed");
            return Err(EngineError::unreadable(internal(error).to_string()));
        }
    };
    if chosen.kind == "model" {
        if answer.discarded || answer.result.text.is_none() {
            engine
                .model_output_failed(&chosen, thread, call, request_id, "handoff")
                .await?;
        } else {
            let mut conn = engine.db().acquire().await?;
            crate::calls::finish_service(
                &mut conn,
                thread,
                orch_provider_contract::calls::Purpose::Summary,
                call,
            )
            .await?;
        }
    }
    measure(
        SummaryField::Handoff,
        if answer.discarded || answer.result.text.is_none() {
            "discarded"
        } else {
            "ok"
        },
    );
    answer
        .result
        .text
        .filter(|_| !answer.discarded)
        .ok_or_else(|| {
            EngineError::unreadable("the producer returned no usable bounded handoff summary")
        })
}

/// Turns of the conversation carried into the new agent's context with its handoff notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    /// The first turn carried.
    pub from: i64,
    /// The last turn carried.
    pub to: i64,
    /// Whether each turn is a few lines (the Summary choice without a summary), or word for word.
    pub brief: bool,
    /// Their written form.
    pub text: String,
}

/// The attributed handoff notice describes the full file separately from a model's fallible summary, and from the
/// turns carried word for word or briefly into the new agent's context.
#[must_use]
pub fn notice(
    thread: Id,
    turns: i64,
    bytes: usize,
    summary: Option<(&str, orch_types::Time)>,
    carried: Option<(&Carried, orch_types::Time)>,
) -> String {
    let path = format!(".orch/handoff/{thread}.md");
    let go_on =
        "Say in a few sentences where the work stands, and go on with what the last message asked.";
    let (first, second, last) = match (summary, carried) {
        (Some(_), _) => (
            "You are continuing this thread's work, which another agent began. None of its conversation is in your context.\nA summary of it follows; it was written by a model and may be wrong in details.".to_owned(),
            format!("The whole transcript is {path} ({turns} turns, {bytes} bytes, one line for each turn at its top): check there before you rely on a detail."),
            "Read that opening and the last 3 turns, and no more for now: go back to an earlier turn when you need it. Then say in a few sentences where the work stands, and go on with what the last message asked.".to_owned(),
        ),
        (None, Some((carried, _))) if carried.brief => (
            format!("You are continuing this thread's work, which another agent began. Of its conversation only turns {} to {} of {turns} are in your context, a few lines each, below.", carried.from, carried.to),
            format!("The whole transcript is {path} ({turns} turns, {bytes} bytes, one line for each turn at its top): read a turn there before you rely on a detail."),
            go_on.to_owned(),
        ),
        (None, Some((carried, _))) if carried.from <= 1 => (
            format!("You are continuing this thread's work, which another agent began. Its whole conversation, {turns} turns, follows word for word."),
            format!("It is also kept as {path} ({bytes} bytes)."),
            go_on.to_owned(),
        ),
        (None, Some((carried, _))) => (
            format!("You are continuing this thread's work, which another agent began. Its most recent turns, {} to {} of {turns}, follow word for word.", carried.from, carried.to),
            format!("The whole transcript is {path} ({turns} turns, {bytes} bytes, one line for each turn at its top): read an earlier turn there when you need it."),
            go_on.to_owned(),
        ),
        (None, None) => (
            "You are continuing this thread's work, which another agent began. None of its conversation is in your context.".to_owned(),
            format!("Its transcript is {path} in your directory: {turns} turns, {bytes} bytes. It opens with one line for each turn."),
            "Read that opening and the last 3 turns, and no more for now: go back to an earlier turn when you need it. Then say in a few sentences where the work stands, and go on with what the last message asked.".to_owned(),
        ),
    };
    let mut text = format!("<orch>\n{first}\n{second}\n{last}\n</orch>");
    if let Some((summary, at)) = summary {
        text.push_str(&format!(
            "\n\n{}",
            crate::instructions::relayed(
                true,
                &thread.to_string(),
                "summary",
                &at.to_string(),
                summary
            )
        ));
    } else if let Some((carried, at)) = carried {
        text.push_str(&format!(
            "\n\n{}",
            crate::instructions::relayed(
                true,
                &thread.to_string(),
                "transcript",
                &at.to_string(),
                &carried.text
            )
        ));
    }
    text
}

#[cfg(test)]
mod bounded_tests {
    use super::*;
    use crate::summaries::budget::budget_of;

    /// A conversation of `turns` turns as the indexed transcript writes it: an index line for each, then the turns.
    fn long_thread(turns: i64) -> IndexedTranscript {
        let index = (1..=turns)
            .map(|turn| {
                format!(
                    "- Turn {turn} · 2026-10-28T10:00:00+00:00 · Ada · {} · completed · line {}",
                    "x".repeat(150),
                    turn * 40
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        IndexedTranscript {
            text: format!("# Conversation index\n\n{index}\n\nthe turns follow"),
            turns,
        }
    }

    fn carried(from: i64, to: i64, bytes: usize) -> Carried {
        Carried {
            from,
            to,
            brief: true,
            text: "b".repeat(bytes),
        }
    }

    #[test]
    fn a_long_thread_is_cut_to_the_share_of_the_models_window_newest_first() {
        // A 4 000-token window at a share of 50 % is 2 000 tokens, which is 6 000 bytes.
        let budget = budget_of(Some(4_000), 50);
        let transcript = long_thread(2_000);
        let recent = carried(1_990, 2_000, brief_bytes(&budget));
        let bounded = bounded_input(&transcript.text, Some(&recent), &budget);
        assert!(
            bounded.text.len() <= budget.bytes,
            "{} bytes of {}",
            bounded.text.len(),
            budget.bytes
        );
        assert_eq!(bounded.brief_turns, 11);
        assert!(bounded.left_out_turns > 0, "the oldest turns have no room");
        assert!(
            bounded.text.contains("- Turn 1989 ·"),
            "the newest older turn is listed"
        );
        assert!(!bounded.text.contains("- Turn 1 ·"), "the oldest is not");
        assert!(
            !bounded.text.contains(" · line "),
            "a pointer into a file the model cannot see is left out"
        );
    }

    #[test]
    fn a_short_thread_is_sent_whole_and_says_nothing_was_left_out() {
        let budget = budget_of(Some(200_000), 50);
        let transcript = long_thread(5);
        let recent = carried(4, 5, 300);
        let bounded = bounded_input(&transcript.text, Some(&recent), &budget);
        assert_eq!(bounded.left_out_turns, 0);
        assert_eq!(bounded.indexed_turns, 3);
        assert!(
            bounded
                .text
                .starts_with("# Newest turns, in brief (turns 4 to 5)")
        );
        assert!(
            bounded
                .text
                .contains("# Older turns, one line each\n- Turn 3 ·")
        );
        assert!(bounded.text.find("- Turn 3 ·") < bounded.text.find("- Turn 1 ·"));
    }

    #[test]
    fn a_share_that_leaves_less_than_the_newest_turn_still_cuts_to_it() {
        // A thousand tokens of share: room beyond the frame, and far less than the newest turn.
        let budget = budget_of(Some(10_000), 10);
        let transcript = long_thread(3);
        let recent = carried(3, 3, 50_000);
        let bounded = bounded_input(&transcript.text, Some(&recent), &budget);
        assert!(bounded.text.len() <= budget.bytes);
        assert!(bounded.text.starts_with("(the start is left out)"));
    }

    #[test]
    fn the_index_is_read_without_its_pointers_and_stops_where_the_turns_begin() {
        let lines = index_lines(&long_thread(3).text);
        assert_eq!(
            lines.iter().map(|(turn, _)| *turn).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(
            lines
                .iter()
                .all(|(_, line)| line.starts_with("- Turn ") && !line.contains(" · line "))
        );
    }
}
