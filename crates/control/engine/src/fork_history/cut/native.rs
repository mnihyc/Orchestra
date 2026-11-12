//! Streamed native comparisons publish one small witness for the frozen source lineage.
use super::{NativeBoundary, NativeRecordEvidence};
use crate::{
    Engine, EngineError,
    commands::{Outcome, Outgoing},
};
use bytes::Bytes;
use futures::{StreamExt as _, stream::BoxStream};
use orch_agent_contract::{
    discovery::RecordedEntry,
    history::{Fidelity, KindCounts},
};
use orch_blob_contract::store::ByteStream;
use orch_db::queries::thread_history_cuts::{self as rows, native as proofs};
use orch_link_contract::commands::{
    AgentReadSession, AgentReadSessionResult, RecordRef, SerialKey,
};
use orch_types::{Hash, Id, Json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(test)]
mod tests;

/// Object key order is immaterial to typed entry equality. Lengths and type tags
/// keep this commitment unambiguous without serializing another whole record.
fn value_digest(value: &Json) -> Hash {
    fn feed(value: &Json, out: &mut impl FnMut(&[u8])) {
        match value {
            Json::Null => out(b"n"),
            Json::Bool(value) => out(if *value { b"t" } else { b"f" }),
            Json::Number(value) => {
                out(b"d");
                let text = if serde_json::Number::from_f64(0.0)
                    .as_ref()
                    .is_some_and(|zero| value == zero)
                {
                    "0.0".to_owned()
                } else {
                    value.to_string()
                };
                out(&(text.len() as u64).to_be_bytes());
                out(text.as_bytes());
            }
            Json::String(value) => {
                out(b"s");
                out(&(value.len() as u64).to_be_bytes());
                out(value.as_bytes());
            }
            Json::Array(values) => {
                out(b"a");
                out(&(values.len() as u64).to_be_bytes());
                for value in values {
                    feed(value, out);
                }
            }
            Json::Object(values) => {
                out(b"o");
                out(&(values.len() as u64).to_be_bytes());
                let mut fields: Vec<_> = values.iter().collect();
                fields.sort_unstable_by(|left, right| left.0.cmp(right.0));
                for (key, value) in fields {
                    out(&(key.len() as u64).to_be_bytes());
                    out(key.as_bytes());
                    feed(value, out);
                }
            }
        }
    }
    let mut hash = Hash::hasher();
    hash.update(b"orch.native-value.v1\0");
    feed(value, &mut |bytes| {
        hash.update(bytes);
    });
    Hash::finish(&hash)
}

fn append(digest: Hash, entry: &RecordedEntry) -> Result<Hash, EngineError> {
    let entry = value_digest(&serde_json::to_value(entry)?);
    let mut bytes = [0_u8; 64];
    bytes[..32].copy_from_slice(digest.as_bytes());
    bytes[32..].copy_from_slice(entry.as_bytes());
    Ok(Hash::of(&bytes))
}

fn empty_digest() -> Hash {
    Hash::of(b"orch.native-record-prefix.v1\0")
}

/// The supplier owns one frame, one line and one parsed entry. The existing whole
/// native-record bound remains, and identity is checked through the actual EOF.
fn lines(
    body: ByteStream,
    record: RecordRef,
) -> BoxStream<'static, Result<Option<RecordedEntry>, EngineError>> {
    Box::pin(futures::stream::try_unfold(
        (
            body,
            Bytes::new(),
            Vec::<u8>::new(),
            Hash::hasher(),
            0_i64,
            false,
        ),
        move |(mut body, mut piece, mut line, mut hash, mut size, mut ended)| async move {
            loop {
                let whole = if let Some(newline) = piece.iter().position(|byte| *byte == b'\n') {
                    let part = piece.split_to(newline + 1);
                    line.extend_from_slice(&part[..newline]);
                    true
                } else {
                    line.extend_from_slice(&piece);
                    piece = Bytes::new();
                    ended && !line.is_empty()
                };
                if whole {
                    let raw = std::mem::take(&mut line);
                    let entry = match std::str::from_utf8(&raw) {
                        Ok(text) if text.trim().is_empty() => continue,
                        Ok(text) => serde_json::from_str::<RecordedEntry>(text).ok(),
                        Err(_) => None,
                    };
                    return Ok(Some((entry, (body, piece, line, hash, size, ended))));
                }
                if ended {
                    return Ok(None);
                }
                match body.next().await {
                    Some(piece_read) => {
                        piece = piece_read.map_err(EngineError::unreadable)?;
                        size = size
                            .checked_add(
                                i64::try_from(piece.len()).map_err(EngineError::unreadable)?,
                            )
                            .ok_or_else(|| {
                                EngineError::unreadable("The native record size is invalid.")
                            })?;
                        if size > record.size
                            || size > orch_agent_contract::discovery::RECORD_MAX_BYTES as i64
                        {
                            return Err(EngineError::unreadable(
                                "The native record exceeds its recorded bound.",
                            ));
                        }
                        hash.update(&piece);
                    }
                    None => {
                        if size != record.size || Hash::finish(&hash) != record.blob {
                            return Err(EngineError::unreadable(
                                "The captured native record failed its identity check.",
                            ));
                        }
                        ended = true;
                    }
                }
            }
        },
    ))
}

async fn record_lines(
    engine: &Engine,
    record: RecordRef,
) -> Result<BoxStream<'static, Result<Option<RecordedEntry>, EngineError>>, EngineError> {
    if record.size < 0 || record.size > orch_agent_contract::discovery::RECORD_MAX_BYTES as i64 {
        return Err(EngineError::unreadable(
            "The retained record exceeds the native-record bound.",
        ));
    }
    let store = engine
        .blobs()
        .ok_or_else(|| EngineError::unreadable("The native record store is unavailable."))?;
    Ok(lines(
        store
            .get(&record.blob, None)
            .await
            .map_err(EngineError::unreadable)?
            .body,
        record,
    ))
}

#[derive(Default)]
struct Coverage {
    entries: i64,
    last: i64,
    invalid: bool,
    kinds: BTreeMap<String, KindCounts>,
}

impl Coverage {
    fn observe(&mut self, entry: &RecordedEntry) -> Result<(), EngineError> {
        let turn = entry.turn();
        if turn <= 0 || (turn != self.last && self.last.checked_add(1) != Some(turn)) {
            self.invalid = true;
        }
        self.last = turn;
        self.entries += 1;
        let kind = match entry {
            RecordedEntry::Said(words) => {
                if words.left_out != 0 {
                    self.invalid = true;
                }
                "user_message".to_owned()
            }
            RecordedEntry::Item(item) => {
                serde_json::from_value::<String>(serde_json::to_value(item.content.kind())?)?
            }
        };
        self.kinds.entry(kind).or_default().kept += 1;
        Ok(())
    }

    fn complete(&self, evidence: &NativeRecordEvidence) -> bool {
        !self.invalid
            && self.entries > 0
            && self.entries == evidence.entries
            && self.last == evidence.turns
            && evidence.turns > 0
            && evidence.report.items_total == self.entries
            && evidence.report.fidelity != Fidelity::Summary
            && evidence.report.dropped.is_empty()
            && self.kinds == evidence.report.by_kind
    }
}

struct FrozenItem {
    id: Id,
    turn: Option<Id>,
    before_orch: bool,
    seq: i64,
    kind: String,
    stream: String,
    payload: Hash,
}

struct Frozen {
    by_name: HashMap<String, Vec<FrozenItem>>,
    before_said: BTreeSet<String>,
    required: BTreeSet<Id>,
    turns: Vec<(Id, i32)>,
}

async fn frozen(engine: &Engine, cut: &rows::Cut) -> Result<Option<Frozen>, EngineError> {
    let (turns, aliases) = {
        let mut conn = engine.db().acquire().await?;
        (
            proofs::turns(&mut conn, cut.id).await?,
            rows::native_aliases(&mut conn, cut.id)
                .await?
                .into_iter()
                .collect::<HashMap<_, _>>(),
        )
    };
    let mut frozen = Frozen {
        by_name: HashMap::new(),
        before_said: BTreeSet::new(),
        required: BTreeSet::new(),
        turns,
    };
    let mut after = 0;
    loop {
        let page = {
            let mut conn = engine.db().acquire().await?;
            rows::items_page(&mut conn, cut.id, after, cut.captured_seq, None).await?
        };
        let Some(last) = page.last() else {
            break;
        };
        after = last.seq_created;
        for item in page {
            if item.before_orch
                && item.kind == "user_message"
                && let Some(name) = item.native_ref.as_ref()
            {
                frozen.before_said.insert(name.clone());
            }
            let required = matches!(item.kind.as_str(), "agent_message" | "tool_call" | "plan");
            let reference = aliases.get(&item.id).or(item.native_ref.as_ref());
            if required {
                if reference.is_none_or(|name| name.starts_with("fork-copy:")) {
                    return Ok(None);
                }
                frozen.required.insert(item.id);
            }
            if let Some(name) = reference {
                frozen
                    .by_name
                    .entry(name.clone())
                    .or_default()
                    .push(FrozenItem {
                        id: item.id,
                        turn: item.turn_id,
                        before_orch: item.before_orch,
                        seq: item.seq_created,
                        payload: value_digest(&item.payload),
                        kind: item.kind,
                        stream: item.stream,
                    });
            }
        }
    }
    Ok(Some(frozen))
}

#[derive(Default)]
struct NativeTurn {
    ordinal: u32,
    witness: bool,
    said: bool,
    unrepresented: bool,
}

struct Proof {
    boundary: NativeBoundary,
    row: proofs::Witness,
    aliases: BTreeMap<Id, String>,
}

async fn compare_source(
    engine: &Engine,
    cut: &rows::Cut,
    record: RecordRef,
    evidence: NativeRecordEvidence,
) -> Result<Option<Proof>, EngineError> {
    let (Some(segment), Some(native)) = (cut.source_segment_id, cut.source_native_ref.as_ref())
    else {
        return Ok(None);
    };
    if cut.first_turn != 1 {
        return Ok(None);
    }
    let (lineage, identity) = {
        let mut conn = engine.db().acquire().await?;
        (
            proofs::lineage(&mut conn, cut.id).await?,
            proofs::identity(&mut conn, cut.id).await?,
        )
    };
    let Some(frozen) = frozen(engine, cut).await? else {
        return Ok(None);
    };
    if frozen.required.is_empty() {
        return Ok(None);
    }
    let mut stream = record_lines(engine, record).await?;
    let mut coverage = Coverage::default();
    let mut digest = empty_digest();
    let mut current = NativeTurn::default();
    let mut ordinals = HashMap::<Id, u32>::new();
    let mut witnesses = BTreeMap::<u32, (i64, String)>::new();
    let mut matched = BTreeMap::<Id, (u32, String)>::new();
    let mut seen = BTreeSet::new();
    let mut prefix: Option<(u32, Hash, i64)> = None;
    let mut invalid_prefix = None;
    let finish_turn = |current: &NativeTurn,
                       digest: Hash,
                       entries: i64,
                       ordinals: &HashMap<Id, u32>,
                       prefix: &mut Option<(u32, Hash, i64)>,
                       invalid: &mut Option<u32>| {
        if current.ordinal == 0 {
            return;
        }
        if current.unrepresented || current.said && !current.witness {
            invalid.get_or_insert(current.ordinal);
        }
        let boundary = frozen.turns.last().map_or(current.witness, |last| {
            ordinals.get(&last.0) == Some(&current.ordinal)
        });
        if boundary {
            *prefix = Some((current.ordinal, digest, entries));
        }
    };
    while let Some(entry) = stream.next().await {
        let Some(entry) = entry? else {
            coverage.invalid = true;
            continue;
        };
        let Ok(ordinal) = u32::try_from(entry.turn()) else {
            return Ok(None);
        };
        if ordinal != current.ordinal {
            finish_turn(
                &current,
                digest,
                coverage.entries,
                &ordinals,
                &mut prefix,
                &mut invalid_prefix,
            );
            current = NativeTurn {
                ordinal,
                ..Default::default()
            };
        }
        coverage.observe(&entry)?;
        digest = append(digest, &entry)?;
        match entry {
            RecordedEntry::Said(words) => {
                if !frozen.before_said.contains(&words.native_ref) {
                    current.said = true;
                }
            }
            RecordedEntry::Item(recorded) => {
                let Some([item]) = frozen.by_name.get(&recorded.native_ref).map(Vec::as_slice)
                else {
                    current.unrepresented = true;
                    continue;
                };
                if !seen.insert(recorded.native_ref.clone()) {
                    return Ok(None);
                }
                let content = serde_json::to_value(&recorded.content)?;
                if content["kind"] != item.kind
                    || value_digest(&content["payload"]) != item.payload
                    || recorded.stream != item.stream
                    || matches!(
                        recorded.content,
                        orch_agent_contract::events::ItemContent::Marker(_)
                    )
                {
                    current.unrepresented = true;
                    continue;
                }
                if matched
                    .insert(item.id, (ordinal, recorded.native_ref.clone()))
                    .is_some()
                {
                    return Ok(None);
                }
                if frozen.required.contains(&item.id) {
                    current.witness = true;
                    let candidate = (item.seq, recorded.native_ref);
                    if witnesses
                        .get(&ordinal)
                        .is_none_or(|old| candidate.0 < old.0)
                    {
                        witnesses.insert(ordinal, candidate);
                    }
                    if let Some(turn) = item.turn {
                        if ordinals
                            .insert(turn, ordinal)
                            .is_some_and(|old| old != ordinal)
                        {
                            return Ok(None);
                        }
                    } else if !item.before_orch {
                        return Ok(None);
                    }
                }
            }
        }
    }
    finish_turn(
        &current,
        digest,
        coverage.entries,
        &ordinals,
        &mut prefix,
        &mut invalid_prefix,
    );
    if !coverage.complete(&evidence) || !frozen.required.iter().all(|id| matched.contains_key(id)) {
        return Ok(None);
    }
    let mut previous = 0;
    for (turn, _) in &frozen.turns {
        let Some(ordinal) = ordinals.get(turn).copied() else {
            return Ok(None);
        };
        if ordinal <= previous {
            return Ok(None);
        }
        previous = ordinal;
    }
    let Some((ordinal, prefix_digest, prefix_entries)) = prefix else {
        return Ok(None);
    };
    if (!frozen.turns.is_empty() && previous != ordinal)
        || invalid_prefix.is_some_and(|bad| bad <= ordinal)
        || frozen.required.iter().any(|id| matched[id].0 > ordinal)
    {
        return Ok(None);
    }
    let Some((_, witness)) = witnesses.remove(&ordinal) else {
        return Ok(None);
    };
    let Ok(record_last) = u32::try_from(coverage.last) else {
        return Ok(None);
    };
    let boundary = NativeBoundary {
        source_segment: segment,
        native_ref: native.clone(),
        ordinal,
        witness,
        record: record.blob,
        record_size: record.size,
        record_last,
        evidence,
    };
    let row = proofs::Witness {
        cut_id: cut.id,
        source_identity: identity.as_bytes().to_vec(),
        lineage: lineage.as_bytes().to_vec(),
        boundary: serde_json::to_value(&boundary)?,
        prefix_digest: prefix_digest.as_bytes().to_vec(),
        prefix_entries,
    };
    Ok(Some(Proof {
        boundary,
        row,
        aliases: matched
            .into_iter()
            .filter(|(_, (turn, _))| *turn <= ordinal)
            .map(|(id, (_, name))| (id, name))
            .collect(),
    }))
}

async fn publish(
    engine: &Engine,
    cut: &rows::Cut,
    proof: Proof,
) -> Result<NativeBoundary, EngineError> {
    crate::side_threads::operation::checkpoint(engine, cut).await?;
    let mut tx = engine.db().begin().await?;
    proofs::lock(&mut tx, cut.id).await?;
    if proofs::lineage(&mut tx, cut.id)
        .await?
        .as_bytes()
        .as_slice()
        != proof.row.lineage.as_slice()
        || proofs::identity(&mut tx, cut.id)
            .await?
            .as_bytes()
            .as_slice()
            != proof.row.source_identity.as_slice()
    {
        return Err(EngineError::unreadable(
            "The native proof belongs to another frozen lineage.",
        ));
    }
    rows::keep_native_aliases(&mut tx, cut.id, &serde_json::to_value(proof.aliases)?).await?;
    if !rows::keep_artifact(
        &mut tx,
        cut.id,
        "native_boundary",
        &proof.row.boundary,
        &[proof.boundary.record],
    )
    .await?
        || !proofs::keep(&mut tx, &proof.row).await?
    {
        return Err(EngineError::unreadable(
            "The native boundary has another retained witness.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(proof.boundary)
}

async fn retained(
    engine: &Engine,
    cut: &rows::Cut,
    boundary: &NativeBoundary,
) -> Result<proofs::Witness, EngineError> {
    if cut.first_turn != 1
        || cut.source_segment_id != Some(boundary.source_segment)
        || cut.source_native_ref.as_deref() != Some(boundary.native_ref.as_str())
        || boundary.record_size < 0
        || boundary.record_size > orch_agent_contract::discovery::RECORD_MAX_BYTES as i64
        || boundary.ordinal == 0
        || boundary.ordinal > boundary.record_last
        || i64::from(boundary.record_last) != boundary.evidence.turns
    {
        return Err(EngineError::unreadable(
            "The retained native witness belongs to another source or boundary.",
        ));
    }
    let existing = {
        let mut conn = engine.db().acquire().await?;
        proofs::witness(&mut conn, cut.id).await?
    };
    let expected = serde_json::to_value(boundary)?;
    if existing.is_none() {
        // An older retained cut is filled once from its actual pinned source, never
        // from a child or a guessed ordinal. Its immutable artifact is not rewritten.
        let proof = compare_source(engine, cut, RecordRef { blob: boundary.record, size: boundary.record_size }, boundary.evidence.clone()).await?
            .filter(|proof| proof.row.boundary == expected)
            .ok_or_else(|| EngineError::unreadable("The retained native record does not certify the complete prefix; its child is retained without replacement."))?;
        publish(engine, cut, proof).await?;
    }
    let mut conn = engine.db().acquire().await?;
    let row = proofs::witness(&mut conn, cut.id).await?.ok_or_else(|| {
        EngineError::unreadable("The retained native comparison witness is absent.")
    })?;
    if row.boundary != expected
        || row.prefix_entries <= 0
        || row.prefix_entries > boundary.evidence.entries
        || row.prefix_digest.len() != 32
        || row.lineage.len() != 32
        || row.source_identity.as_slice()
            != proofs::identity(&mut conn, cut.id)
                .await?
                .as_bytes()
                .as_slice()
    {
        return Err(EngineError::unreadable(
            "The native witness no longer certifies this frozen lineage; its child is retained without replacement.",
        ));
    }
    Ok(row)
}

pub(crate) async fn native_boundary(
    engine: &Engine,
    cut: &rows::Cut,
    machine: Id,
    provider: Id,
    agent: Id,
    command: Id,
) -> Result<Option<NativeBoundary>, EngineError> {
    crate::side_threads::operation::checkpoint(engine, cut).await?;
    if let Some(old) = cut.artifacts.get("native_boundary") {
        let boundary: NativeBoundary = serde_json::from_value(old.clone()).map_err(|_| EngineError::unreadable("The retained native witness lacks complete typed record evidence; another child is not admitted."))?;
        retained(engine, cut, &boundary).await?;
        crate::side_threads::operation::checkpoint(engine, cut).await?;
        return Ok(Some(boundary));
    }
    if cut.first_turn != 1 {
        return Ok(None);
    }
    let Some(native) = cut
        .source_native_ref
        .as_ref()
        .filter(|_| cut.source_segment_id.is_some())
    else {
        return Ok(None);
    };
    {
        let mut conn = engine.db().acquire().await?;
        let limit = crate::fork_history::native_proof_limit(&mut conn, cut.thread_id).await?;
        let counted = rows::counts(&mut conn, cut.id).await?;
        if counted.items.values().sum::<i64>() > crate::fork_history::native_proof_items(limit)
            || rows::payload_bytes(&mut conn, cut.id).await? > limit
        {
            return Ok(None);
        }
    }
    let proposed = Outgoing::new(
        command,
        machine,
        SerialKey::Harness {
            provider_id: provider,
            agent_id: agent,
        },
        &AgentReadSession {
            provider_id: provider,
            agent_id: agent,
            native_session_ref: native.clone(),
            write: true,
        },
    )?;
    let proposed = crate::side_threads::command::source_record(engine, cut, proposed).await?;
    let read: AgentReadSessionResult = match crate::commands::run(
        engine.inner.commands.as_ref(),
        proposed,
    )
    .await?
    {
        Outcome::Succeeded(value) => serde_json::from_value(value)?,
        Outcome::Failed(_) => return Ok(None),
        _ => {
            return Err(EngineError::unreadable(
                "The original native record outcome is unconfirmed; no native boundary is guessed.",
            ));
        }
    };
    let Some(record) = read.record.filter(|record| {
        record.size >= 0 && record.size <= orch_agent_contract::discovery::RECORD_MAX_BYTES as i64
    }) else {
        return Ok(None);
    };
    let Some(proof) =
        compare_source(engine, cut, record, NativeRecordEvidence::from(&read)).await?
    else {
        return Ok(None);
    };
    Ok(Some(publish(engine, cut, proof).await?))
}

/// Child readback consumes the entire ordered record and its report. Only the
/// already proved source prefix's commitment is reused, never a second child.
pub(crate) async fn verify_native_record(
    engine: &Engine,
    cut: &rows::Cut,
    boundary: &NativeBoundary,
    child: &AgentReadSessionResult,
) -> Result<Hash, EngineError> {
    crate::side_threads::operation::checkpoint(engine, cut).await?;
    let proof = retained(engine, cut, boundary).await?;
    let record = child.record.ok_or_else(|| {
        EngineError::unreadable("The owned native child has no complete record readback.")
    })?;
    let mut stream = record_lines(engine, record).await?;
    let mut coverage = Coverage::default();
    let mut digest = empty_digest();
    while let Some(entry) = stream.next().await {
        let Some(entry) = entry? else {
            coverage.invalid = true;
            continue;
        };
        coverage.observe(&entry)?;
        digest = append(digest, &entry)?;
    }
    if !coverage.complete(&NativeRecordEvidence::from(child))
        || child.turns != i64::from(boundary.ordinal)
    {
        return Err(EngineError::unreadable(
            "The native record/report does not certify the complete prefix from native turn1; the successful child is retained without replacement.",
        ));
    }
    if coverage.entries != proof.prefix_entries
        || digest.as_bytes().as_slice() != proof.prefix_digest.as_slice()
    {
        return Err(EngineError::unreadable(
            "The owned child's complete native record differs from the confirmed source prefix; no activation or replacement child was admitted.",
        ));
    }
    crate::side_threads::operation::checkpoint(engine, cut).await?;
    Ok(record.blob)
}
