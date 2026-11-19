//! Read-only fingerprints of the complete frozen native lineage.

use super::*;
use orch_types::Hash;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Position {
    pub thread_id: String,
    pub end_ordinal_exclusive: u64,
    pub end_byte_offset: u64,
}

#[derive(Clone)]
pub(super) struct Segment {
    pub id: String,
    pub path: PathBuf,
    pub header_fingerprint: Hash,
    pub paginated: bool,
    pub base: Option<Position>,
    pub end: Option<Position>,
}

fn reader(path: &Path) -> Result<Box<dyn BufRead + Send>, AdapterError> {
    let file = std::fs::File::open(path)
        .map_err(|_| invalid("A retained native history range is missing or unreadable."))?;
    // Bound each read to the opened file's current bytes, even if a later
    // native operation appends after this read-only inspection was cancelled.
    let length = file
        .metadata()
        .map_err(|_| invalid("The native history range has no readable length."))?
        .len();
    let file = file.take(length);
    let input: Box<dyn Read + Send> =
        if path.extension().is_some_and(|extension| extension == "zst") {
            Box::new(
                zstd::stream::read::Decoder::new(file)
                    .map_err(|_| invalid("A compressed native history range cannot be read."))?,
            )
        } else {
            Box::new(file)
        };
    Ok(Box::new(BufReader::new(input)))
}

fn line(input: &mut dyn BufRead, bytes: &mut Vec<u8>) -> Result<usize, AdapterError> {
    bytes.clear();
    let read = input
        .take(protocol::MAX_FRAME_BYTES as u64 + 1)
        .read_until(b'\n', bytes)
        .map_err(|_| invalid("A retained native history range cannot be read."))?;
    if bytes.len() > protocol::MAX_FRAME_BYTES {
        return Err(invalid(
            "A native history entry exceeds the harness frame limit.",
        ));
    }
    if read > 0 && bytes.last() != Some(&b'\n') {
        return Err(invalid("The native history has an incomplete record."));
    }
    Ok(read)
}

fn mode(header: &Json) -> Result<&str, AdapterError> {
    match header["payload"].get("history_mode") {
        None => Ok("legacy"),
        Some(Json::String(value)) if matches!(value.as_str(), "legacy" | "paginated") => Ok(value),
        _ => Err(unsupported(
            "The native history declares an unknown persistence mode.",
        )),
    }
}

pub(super) fn segment(
    home: &Path,
    path: &Path,
    id: &str,
    end: Option<Position>,
) -> Result<Segment, AdapterError> {
    let plain = if path.extension().is_some_and(|extension| extension == "zst") {
        path.with_extension("")
    } else {
        path.to_path_buf()
    };
    let candidate = if plain.is_file() {
        plain
    } else {
        PathBuf::from(format!("{}.zst", plain.display()))
    };
    let path = candidate
        .canonicalize()
        .map_err(|_| invalid("A retained native history range is missing."))?;
    if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        return Err(invalid(
            "A retained native history range is not a regular file.",
        ));
    }
    let owned = [home.join("sessions"), home.join("archived_sessions")]
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| path.starts_with(root));
    if !owned {
        return Err(invalid(
            "The native history is outside this harness's retained session directories.",
        ));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("The native rollout has no valid filename."))?;
    let suffix = format!("-{id}.jsonl");
    if !name.strip_suffix(".zst").unwrap_or(name).ends_with(&suffix) {
        return Err(unsupported(
            "This native source uses a replaced rollout identity; its rollback history cannot provide this checkpoint.",
        ));
    }
    let mut input = reader(&path)?;
    let mut bytes = Vec::new();
    if line(input.as_mut(), &mut bytes)? == 0 {
        return Err(invalid("The native history is empty."));
    }
    let header: Json = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("The native session metadata is invalid."))?;
    if header["type"] != "session_meta" || header["payload"]["id"] != id {
        return Err(invalid(
            "A retained native range belongs to another session.",
        ));
    }
    if header["payload"]["cli_version"] != "0.165.0" {
        return Err(unsupported(
            "The retained history was created by a different Codex version.",
        ));
    }
    if header["payload"]
        .get("subagent_history_start_ordinal")
        .is_some_and(|value| !value.is_null())
    {
        return Err(unsupported(
            "A native subagent's hidden inherited inputs do not provide this checkpoint.",
        ));
    }
    let base = match header["payload"].get("history_base") {
        None | Some(Json::Null) => None,
        Some(value) => Some(
            serde_json::from_value::<Position>(value.clone())
                .map_err(|_| invalid("A retained native ancestry boundary is invalid."))?,
        ),
    };
    let paginated = mode(&header)? == "paginated";
    if !paginated && base.is_some() {
        return Err(invalid(
            "Legacy native history cannot name paginated ancestry.",
        ));
    }
    if let Some(base) = &base
        && (base.thread_id.parse::<orch_types::Id>().is_err()
            || base.end_ordinal_exclusive == 0
            || base.end_byte_offset == 0)
    {
        return Err(invalid("A retained native ancestry boundary is invalid."));
    }
    Ok(Segment {
        id: id.into(),
        path,
        header_fingerprint: Hash::of(&bytes),
        paginated,
        base,
        end,
    })
}

fn field(hash: &mut blake3::Hasher, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

#[derive(Default)]
struct Scan {
    cut: Option<(String, u64, u64)>,
    empty: bool,
    context: bool,
    active: Option<String>,
    users: HashSet<(String, String)>,
    turn_users: HashMap<String, usize>,
    matched: bool,
    matched_id: Option<String>,
}

fn unsafe_record(row: &Json) -> bool {
    row["type"] == "compacted"
        || (row["type"] == "event_msg"
            && matches!(
                row["payload"]["type"].as_str(),
                Some("context_compacted" | "thread_rolled_back")
            ))
}

impl Scan {
    fn observe(
        &mut self,
        row: &Json,
        selection: &Selection,
        id: &str,
        offset: u64,
        ordinal: u64,
    ) -> Result<(), AdapterError> {
        if unsafe_record(row) {
            return Err(unsupported(
                "This native source contains compaction or rollback; it does not provide a verified before-input checkpoint.",
            ));
        }
        let event = &row["payload"];
        if row["type"] == "event_msg" {
            match event["type"].as_str() {
                Some("task_started") => {
                    let turn = event["turn_id"]
                        .as_str()
                        .ok_or_else(|| invalid("A native turn start has no identity."))?;
                    if turn == selection.turn {
                        if self.cut.is_some() {
                            return Err(invalid(
                                "The native history repeats the selected turn's start.",
                            ));
                        }
                        if self.active.is_some() {
                            return Err(unsupported(
                                "The native prefix contains an unfinished preceding turn.",
                            ));
                        }
                        self.empty = !self.context;
                        self.cut = Some((id.into(), offset, ordinal));
                    }
                    self.active = Some(turn.into());
                }
                Some("task_complete" | "turn_aborted") => {
                    self.active = None;
                }
                _ => {}
            }
            if let Some((item, client)) = native_user_event(row) {
                let turn = event["turn_id"]
                    .as_str()
                    .or(self.active.as_deref())
                    .ok_or_else(|| invalid("A native user item has no owning turn."))?;
                let identity = client
                    .or_else(|| item["id"].as_str())
                    .ok_or_else(|| invalid("A native user input has no retained identity."))?;
                let first = self.users.insert((turn.into(), identity.into()));
                if first {
                    *self.turn_users.entry(turn.into()).or_default() += 1;
                }
                if selection.matches(item, client) {
                    if turn != selection.turn
                        || self.cut.is_none()
                        || self.turn_users.get(turn) != Some(&1)
                    {
                        return Err(unsupported(
                            "Codex forks at native turn boundaries; this input was added inside its turn.",
                        ));
                    }
                    if let Some(id) = item["id"].as_str() {
                        if self.matched_id.as_deref().is_some_and(|prior| prior != id) {
                            return Err(invalid(
                                "The retained native input has ambiguous item identities.",
                            ));
                        }
                        self.matched_id = Some(id.into());
                    }
                    self.matched = true;
                }
            }
        }
        if self.cut.is_none() && row["type"] != "session_meta" {
            self.context = true;
        }
        Ok(())
    }
}

pub(super) fn fingerprint(
    segments: &[Segment],
    selection: &Selection,
    source: &NativePrefixSource,
) -> Result<(Hash, bool), AdapterError> {
    if segments
        .last()
        .is_none_or(|segment| segment.id != source.native_session_ref)
    {
        return Err(invalid(
            "The native lineage does not belong to the requested source.",
        ));
    }
    let mut seen = HashSet::new();
    for (index, segment) in segments.iter().enumerate() {
        if !seen.insert(&segment.id) {
            return Err(invalid("The native inherited history contains a cycle."));
        }
        if index == 0 {
            if segment.base.is_some() {
                return Err(invalid("A native inherited ancestor is missing."));
            }
        } else if segment.base.as_ref().is_none_or(|base| {
            base.thread_id != segments[index - 1].id
                || Some(base) != segments[index - 1].end.as_ref()
        }) {
            return Err(invalid(
                "The native inherited ranges do not match their declared ancestry.",
            ));
        }
    }
    let mut hash = Hash::hasher();
    hash.update(b"orch-codex-before-turn-v1\0");
    field(&mut hash, source.native_session_ref.as_bytes());
    field(&mut hash, selection.turn.as_bytes());
    field(&mut hash, selection.item.as_bytes());
    field(&mut hash, selection.identity().as_bytes());
    let mut scan = Scan::default();
    for segment in segments {
        let mut input = reader(&segment.path)?;
        let mut bytes = Vec::new();
        let mut offset = 0u64;
        let mut expected = None;
        let mut first = true;
        let mut range = Hash::hasher();
        let mut used = 0u64;
        let paginated = segment.paginated;
        loop {
            if segment
                .end
                .as_ref()
                .is_some_and(|end| offset == end.end_byte_offset)
            {
                break;
            }
            let before = offset;
            let read = line(input.as_mut(), &mut bytes)?;
            if read == 0 {
                break;
            }
            offset = offset
                .checked_add(read as u64)
                .ok_or_else(|| invalid("A native history offset overflows."))?;
            if segment
                .end
                .as_ref()
                .is_some_and(|end| offset > end.end_byte_offset)
            {
                return Err(invalid("A native ancestry cutoff splits a history record."));
            }
            let row: Json = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("The native history contains an invalid record."))?;
            let ordinal = if paginated {
                row["ordinal"]
                    .as_u64()
                    .ok_or_else(|| invalid("A paginated native record has no ordinal."))?
            } else {
                expected.unwrap_or(0)
            };
            if first {
                if Hash::of(&bytes) != segment.header_fingerprint {
                    return Err(conflict(
                        "Native ancestry metadata changed during inspection.",
                    ));
                }
                let start = segment
                    .base
                    .as_ref()
                    .map_or(0, |base| base.end_ordinal_exclusive);
                if paginated && ordinal != start {
                    return Err(invalid(
                        "A native range starts at another ancestry ordinal.",
                    ));
                }
                field(&mut hash, segment.id.as_bytes());
                field(&mut hash, segment.header_fingerprint.as_bytes());
                first = false;
            } else {
                if expected != Some(ordinal) {
                    return Err(invalid("Native history ordinals are not contiguous."));
                }
                scan.observe(&row, selection, &segment.id, before, ordinal)?;
            }
            expected = ordinal.checked_add(1);
            if expected.is_none() {
                return Err(invalid("A native history ordinal overflows."));
            }
            if scan.cut.is_none() {
                range.update(&bytes);
                used = offset;
            }
        }
        if first {
            return Err(invalid(
                "A retained native ancestor is missing its metadata.",
            ));
        }
        if let Some(end) = &segment.end
            && (end.thread_id != segment.id
                || offset != end.end_byte_offset
                || expected != Some(end.end_ordinal_exclusive))
        {
            return Err(invalid(
                "A native ancestor is truncated or its frozen range changed.",
            ));
        }
        field(&mut hash, &used.to_le_bytes());
        field(&mut hash, Hash::finish(&range).as_bytes());
    }
    let Some((owner, offset, ordinal)) = scan.cut else {
        return Err(invalid(
            "The selected native input has no retained turn-start checkpoint.",
        ));
    };
    if !scan.matched {
        return Err(invalid(
            "The API input is not retained in its declared native turn.",
        ));
    }
    field(
        &mut hash,
        scan.matched_id
            .as_deref()
            .unwrap_or(selection.identity())
            .as_bytes(),
    );
    field(&mut hash, owner.as_bytes());
    field(&mut hash, &offset.to_le_bytes());
    field(&mut hash, &ordinal.to_le_bytes());
    Ok((Hash::finish(&hash), scan.empty))
}
