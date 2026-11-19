//! Controlled-model history witnesses for native input-prefix proofs.

use orch_types::Json;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

fn container_mode(value: Option<&OsStr>, module: &str, marker: bool) -> Result<bool, &'static str> {
    let Some(value) = value else {
        return Ok(false);
    };
    if value != OsStr::new("1") {
        return Err("ORCH_PREFIX_PROOF_IN_CONTAINER accepts only 1 when set");
    }
    if module.split("::").next() != Some("input_prefix") {
        return Err("container prefix proof mode is restricted to the input_prefix test target");
    }
    if !marker {
        return Err(
            "container prefix proof mode requires the actual Linux Docker container marker",
        );
    }
    Ok(true)
}

/// Opts only the prefix integration target into direct execution inside its
/// already-owned Docker container. CLI pins are checked by the fixture's real
/// adapter startup, under its existing probe timeout, before opening a session.
#[must_use]
pub fn container_prefix_proof_mode(module: &str) -> bool {
    let value = std::env::var_os("ORCH_PREFIX_PROOF_IN_CONTAINER");
    let marker = value.is_some() && cfg!(target_os = "linux") && Path::new("/.dockerenv").is_file();
    container_mode(value.as_deref(), module, marker).unwrap_or_else(|reason| panic!("{reason}"))
}

/// Finds the native argv within either the CLI itself or this proof's exact
/// transparent stdin gate. Other wrappers have different process/fault semantics
/// and must not be treated as equivalent to the native container proof.
pub fn prefix_native_args_offset(
    program: &str,
    args: &[String],
    native: &str,
    roots: &[PathBuf],
) -> Result<usize, &'static str> {
    if program == native && !args.is_empty() {
        return Ok(0);
    }
    if program != "node" || args.len() < 3 || args[1] != native {
        return Err("container prefix proof refuses this special launcher shape");
    }
    let gate = Path::new(&args[0]);
    if !gate.is_absolute() || gate.file_name() != Some(OsStr::new("prefix-wire-gate.cjs")) {
        return Err("container prefix proof accepts only its own transparent stdin gate");
    }
    let gate = gate
        .canonicalize()
        .map_err(|_| "the prefix stdin gate is not retained")?;
    if !roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| gate.starts_with(root))
    {
        return Err("the prefix stdin gate must belong to a generated fixture directory");
    }
    let mut bytes = Vec::new();
    std::fs::File::open(gate)
        .map_err(|_| "the prefix stdin gate cannot be read")?
        .take(STEER_WIRE_GATE.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "the prefix stdin gate cannot be read")?;
    if bytes != STEER_WIRE_GATE.as_bytes() {
        return Err("container prefix proof refuses a modified or fault-injecting stdin gate");
    }
    Ok(2)
}

/// A native close may append its administrative prompt/mode/cost checkpoint. Every
/// pre-existing byte must stay identical, and no added conversation/tool entry
/// or foreign session is accepted as an administrative append.
pub fn claude_source_preserved(
    before: &[u8],
    after: &[u8],
    session: &str,
) -> Result<usize, &'static str> {
    if !after.starts_with(before) {
        return Err("the native source changed or removed original bytes");
    }
    let mut leaf = None;
    for line in before
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let row: Json = serde_json::from_slice(line)
            .map_err(|_| "the original source has an invalid record")?;
        if matches!(row["type"].as_str(), Some("user" | "assistant"))
            && row["isSidechain"] != true
            && row["isMeta"] != true
            && let Some(id) = row["uuid"].as_str()
        {
            leaf = Some(id.to_owned());
        }
    }
    let mut appended = 0;
    for line in after[before.len()..]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let row: Json = serde_json::from_slice(line)
            .map_err(|_| "the native source has an incomplete administrative append")?;
        if row["sessionId"] != session {
            return Err("an administrative append names another native session");
        }
        let allowed: &[&str] = match row["type"].as_str() {
            Some("last-prompt") => {
                if row["leafUuid"].as_str() != leaf.as_deref()
                    || leaf.is_none()
                    || !row["lastPrompt"].is_string()
                {
                    return Err("the native last-prompt append has no original leaf checkpoint");
                }
                &["type", "sessionId", "lastPrompt", "leafUuid"]
            }
            // The permission mode the session ended in: the pinned harness writes it at
            // some closes only.
            Some("mode") => {
                if !row["mode"].is_string() {
                    return Err("the native mode append names no mode");
                }
                &["type", "sessionId", "mode"]
            }
            Some("cost-state") => &[
                "type",
                "sessionId",
                "hasUnknownModelCost",
                "modelUsage",
                "startTime",
                "totalAPIDuration",
                "totalAPIDurationWithoutRetries",
                "totalCostUSD",
                "totalDuration",
                "totalLinesAdded",
                "totalLinesRemoved",
                "totalToolDuration",
            ],
            _ => {
                return Err(
                    "the native source appended conversation or tool material during preparation",
                );
            }
        };
        if !row
            .as_object()
            .is_some_and(|row| row.keys().all(|key| allowed.contains(&key.as_str())))
        {
            return Err("an administrative append contains other native material");
        }
        appended += 1;
    }
    Ok(appended)
}

fn evidence_bytes(bytes: &[u8], secrets: &[&str], limit: usize) -> std::io::Result<Vec<u8>> {
    if bytes.len() > limit {
        return Err(std::io::Error::other(
            "prefix evidence exceeds the native frame budget",
        ));
    }
    let mut text = std::str::from_utf8(bytes)
        .map_err(|_| std::io::Error::other("prefix JSON evidence is not UTF-8"))?
        .to_owned();
    for &secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        text = text.replace(secret, "<synthetic credential omitted>");
    }
    if text.len() > limit {
        return Err(std::io::Error::other(
            "redacted prefix evidence exceeds the native frame budget",
        ));
    }
    Ok(text.into_bytes())
}

fn keep_evidence_file(
    source: &Path,
    destination: &Path,
    secrets: &[&str],
    limit: usize,
) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "prefix evidence must be an owned regular file",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(source)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    let kept = evidence_bytes(&bytes, secrets, limit)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(destination, kept)
}

/// Retains only synthetic native histories and model bodies, never a home tree,
/// auth/config/SQLite file, request header or environment. The frame ceiling is
/// the existing native fixture's. The owned evidence directory survives TempDir
/// cleanup and is copied out by the root executor after a failed proof.
pub fn preserve_prefix_failure(
    kind: &str,
    home: &Path,
    work: &Path,
    extra: &[PathBuf],
    bodies: &[Json],
    secrets: &[&str],
    limit: usize,
) -> std::io::Result<PathBuf> {
    let temporary = tempfile::Builder::new()
        .prefix("orch-prefix-evidence-")
        .tempdir()?;
    let output = temporary.keep();
    eprintln!("native prefix evidence: {}", output.display());
    let roots: &[&str] = match kind {
        "codex" => &["sessions", "archived_sessions", ".orch-input-prefixes"],
        "claude" => &["projects"],
        _ => return Err(std::io::Error::other("unknown native prefix evidence kind")),
    };
    let mut pending = Vec::new();
    for (name, kept) in [
        ("prefix-source-before.jsonl", "source-before.jsonl"),
        ("prefix-proof.json", "proof.json"),
    ] {
        let path = home.join(name);
        if path.is_file() {
            keep_evidence_file(&path, &output.join(kept), secrets, limit)?;
        }
    }
    for root in roots {
        let path = home.join(root);
        if path.is_dir() {
            pending.push(path);
        }
    }
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() && path.extension() == Some(OsStr::new("jsonl")) {
                let relative = path.strip_prefix(home).map_err(|_| {
                    std::io::Error::other("prefix evidence escaped its generated home")
                })?;
                keep_evidence_file(&path, &output.join("native").join(relative), secrets, limit)?;
            }
        }
    }
    for (index, path) in extra.iter().enumerate() {
        if path.is_file() {
            keep_evidence_file(
                path,
                &output.join(format!("native-api-{index}.jsonl")),
                secrets,
                limit,
            )?;
        }
    }
    for (index, body) in bodies.iter().enumerate() {
        let bytes = serde_json::to_vec(body).map_err(std::io::Error::other)?;
        std::fs::write(
            output.join(format!("model-body-{index}.json")),
            evidence_bytes(&bytes, secrets, limit)?,
        )?;
    }
    for name in [
        "prefix-tool-runs",
        "prefix-tool-result",
        "prefix-steer-sent",
        "prefix-steer-echoed",
        "prefix-current-file",
    ] {
        let path = work.join(name);
        if path.is_file() {
            // These are fixed generated fixture bytes, including the binary sentinel.
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > limit {
                return Err(std::io::Error::other(
                    "prefix workspace witness exceeds the native frame budget",
                ));
            }
            std::fs::write(output.join(name), bytes)?;
        }
    }
    Ok(output)
}

/// A test-only transparent native stdin gate. Original byte chunks go to the
/// real pinned CLI unchanged. Separate sent/echoed sentinels distinguish pipe
/// acceptance from the actual native user UUID echo. No payload, environment or
/// credential is logged.
pub const STEER_WIRE_GATE: &str = r#"
const fs = require('node:fs');
const cp = require('node:child_process');
const { StringDecoder } = require('node:string_decoder');
const child = cp.spawn(process.argv[2], process.argv.slice(3), {stdio:['pipe','pipe','inherit'], env:process.env});
const decoder = new StringDecoder('utf8');
let buffered = '';
let steerUuid = null;
const outputDecoder = new StringDecoder('utf8');
let output = '';
child.stdout.pipe(process.stdout);
child.stdout.on('data', chunk => {
  output += outputDecoder.write(chunk);
  for (;;) {
    const end = output.indexOf('\n');
    if (end < 0) break;
    const line = output.slice(0,end); output = output.slice(end+1);
    try {
      const value = JSON.parse(line);
      if (steerUuid && value.type === 'user' && value.uuid === steerUuid && process.env.ORCH_PREFIX_WIRE_ACK)
        fs.writeFileSync(process.env.ORCH_PREFIX_WIRE_ACK, value.uuid);
    } catch (_) {}
  }
});
process.stdin.on('data', chunk => {
  buffered += decoder.write(chunk);
  let steer = false;
  for (;;) {
    const end = buffered.indexOf('\n');
    if (end < 0) break;
    const line = buffered.slice(0,end); buffered = buffered.slice(end+1);
    try {
      const value = JSON.parse(line);
      if ((value.method === 'turn/steer' || value.type === 'user') && line.includes('PREFIX-STEER-USER')) steer = true;
      if (value.type === 'user' && line.includes('PREFIX-STEER-USER') && typeof value.uuid === 'string') steerUuid = value.uuid;
    } catch (_) {}
  }
  child.stdin.write(chunk, error => {
    if (error) { process.exitCode=1; return; }
    if (steer) fs.writeFileSync(process.env.ORCH_PREFIX_WIRE_SENT, steerUuid || 'sent');
  });
});
process.stdin.on('end', () => child.stdin.end());
child.stdin.on('error', () => { process.exitCode=1; });
child.on('error', () => process.exit(1));
child.on('close', code => {
  process.exitCode = code === null ? 1 : code;
  process.stdin.destroy();
});
"#;

fn cache_hint(value: &Json) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("type").and_then(Json::as_str) == Some("ephemeral")
            && object
                .keys()
                .all(|key| matches!(key.as_str(), "type" | "ttl"))
            && object
                .get("ttl")
                .is_none_or(|ttl| matches!(ttl.as_str(), Some("5m" | "1h")))
    })
}

fn remove_cache_hint(value: &mut Json) {
    if value.get("cache_control").is_some_and(cache_hint)
        && let Some(object) = value.as_object_mut()
    {
        object.remove("cache_control");
    }
}

fn normalize_content(value: &mut Json) {
    let Some(blocks) = value.as_array_mut() else {
        return;
    };
    for block in blocks {
        if matches!(
            block["type"].as_str(),
            Some(
                "text"
                    | "image"
                    | "document"
                    | "tool_use"
                    | "tool_result"
                    | "thinking"
                    | "redacted_thinking"
            )
        ) {
            remove_cache_hint(block);
            if block["type"] == "tool_result"
                && let Some(content) = block.get_mut("content")
            {
                normalize_content(content);
            }
        }
    }
}

/// Full model-visible input for the controlled prefix cases. Exactly one
/// authored text block is substituted in place, including when native context
/// follows it. All message/block order, unknown content, tool arguments,
/// results, signatures and IDs remain. Known Messages cache hints describe
/// reuse, not content; no application field inside tool arguments is removed.
/// An opaque server-side history reference is not a full-history witness.
pub fn full_model_input(body: &Json, authored: &str) -> Result<Json, &'static str> {
    Ok(model_input(body, authored)?.0)
}

fn model_input(body: &Json, authored: &str) -> Result<(Json, usize), &'static str> {
    if authored.is_empty() {
        return Err("the controlled authored input must be nonempty");
    }
    for key in ["previous_response_id", "conversation"] {
        if body.get(key).is_some_and(|value| !value.is_null()) {
            return Err(
                "the model proof has an opaque history reference instead of complete input",
            );
        }
    }
    let key = match (body.get("messages"), body.get("input")) {
        (Some(_), None) => "messages",
        (None, Some(_)) => "input",
        _ => return Err("the model proof requires exactly one complete input array"),
    };
    let mut rows = body[key]
        .as_array()
        .ok_or("the model input is not an array")?
        .clone();
    let mut matched = 0;
    let mut selected = 0;
    for (index, row) in rows.iter_mut().enumerate() {
        let user = row["role"] == "user";
        if let Some(content) = row.get_mut("content") {
            if key == "messages" {
                normalize_content(content);
            }
            if user {
                if content.as_str() == Some(authored) {
                    *content = Json::String("<controlled authored input>".into());
                    matched += 1;
                    selected = index;
                } else if let Some(blocks) = content.as_array_mut() {
                    for block in blocks {
                        if matches!(block["type"].as_str(), Some("text" | "input_text"))
                            && block["text"].as_str() == Some(authored)
                        {
                            block["text"] = Json::String("<controlled authored input>".into());
                            matched += 1;
                            selected = index;
                        }
                    }
                }
            }
        }
    }
    if matched != 1 {
        return Err("the controlled input is not one exact authored user text block");
    }
    let mut input = serde_json::Map::new();
    input.insert(key.into(), Json::Array(rows));
    for field in ["system", "instructions", "tools", "tool_choice"] {
        if let Some(value) = body.get(field) {
            let mut value = value.clone();
            if key == "messages" {
                if field == "system" {
                    normalize_content(&mut value);
                }
                if field == "tools"
                    && let Some(tools) = value.as_array_mut()
                {
                    for tool in tools {
                        remove_cache_hint(tool);
                    }
                }
            }
            input.insert(field.into(), value);
        }
    }
    Ok((Json::Object(input), selected))
}

fn codex_envelope_id(
    row: &mut Json,
    version: usize,
    replacement: &str,
) -> Result<(), &'static str> {
    if row["type"] != "message"
        || !row["id"]
            .as_str()
            .and_then(|id| id.strip_prefix("msg_"))
            .and_then(|id| id.parse::<orch_types::Id>().ok())
            .is_some_and(|id| id.get_version_num() == version)
    {
        return Err("the pinned Codex message has an unfamiliar transport envelope identity");
    }
    row["id"] = Json::String(replacement.into());
    Ok(())
}

/// Codex 0.162.1 rebuilds its first developer envelope with a thread-scoped
/// UUIDv5, and assigns a UUIDv7 to the new authored message. Those two envelope
/// IDs are not retained conversation identity. Every old message/tool ID and
/// reference remains exact. A proved empty native prefix also regenerates its
/// initial text context through the normal missing-reference-context path;
/// only that explicit case permits its new context envelope IDs to differ.
/// Content and order, including anything following the authored input, stay.
pub fn codex_model_input(
    body: &Json,
    authored: &str,
    empty_prefix: bool,
) -> Result<Json, &'static str> {
    let (mut input, selected) = model_input(body, authored)?;
    let rows = input
        .get_mut("input")
        .and_then(Json::as_array_mut)
        .ok_or("the Codex proof requires a complete Responses input array")?;
    if selected == 0 || rows[0]["role"] != "developer" || rows[selected]["role"] != "user" {
        return Err(
            "the pinned Codex request lacks its developer prefix or selected user envelope",
        );
    }
    codex_envelope_id(&mut rows[0], 5, "<native thread developer envelope>")?;
    codex_envelope_id(&mut rows[selected], 7, "<new authored envelope>")?;
    if empty_prefix {
        for row in &mut rows[1..selected] {
            if !matches!(row["role"].as_str(), Some("developer" | "user"))
                || !row["content"].as_array().is_some_and(|blocks| {
                    !blocks.is_empty()
                        && blocks
                            .iter()
                            .all(|block| block["type"] == "input_text" && block["text"].is_string())
                })
            {
                return Err(
                    "an empty Codex prefix contains material other than fresh initial text context",
                );
            }
            codex_envelope_id(row, 7, "<fresh initial context envelope>")?;
        }
    }
    Ok(input)
}

fn claude_first_input_fingerprint(text: &str) -> String {
    // The pinned CLI samples JavaScript string indices (UTF-16 code units),
    // joins them, then hashes their UTF-8 encoding with this public constant.
    let sampled = [4, 7, 20].map(|index| text.encode_utf16().nth(index).unwrap_or(u16::from(b'0')));
    let sampled = String::from_utf16_lossy(&sampled);
    let preimage = format!("59cf53e54c78{sampled}2.1.296");
    let digest = ring::digest::digest(&ring::digest::SHA256, preimage.as_bytes());
    let bytes = digest.as_ref();
    format!("{:02x}{:x}", bytes[0], bytes[1] >> 4)
}

/// Only for a proved empty Claude Code 2.1.296 prefix: its first authored input
/// changes the native billing fingerprint. Validate the exact pinned header
/// and derived value before replacing only its three checksum bytes. All other
/// system content/fields stay exact. The caller independently verifies the raw
/// first native user identity/text; this does not identify a native checkpoint.
/// Ordinary middle/queued cases must keep using the general strict oracle.
pub fn claude_first_model_input(body: &Json, authored: &str) -> Result<Json, &'static str> {
    const PREFIX: &str = "x-anthropic-billing-header: cc_version=2.1.296.";
    const SUFFIX: &str = "; cc_entrypoint=sdk-cli;";
    let (mut input, selected) = model_input(body, authored)?;
    if selected != 0 || input.get("messages").is_none() {
        return Err(
            "the first-input billing exemption requires the actual first Messages user row",
        );
    }
    let system = input
        .get("system")
        .and_then(Json::as_array)
        .ok_or("the pinned Claude first-input request has no system block array")?;
    let header = system
        .first()
        .ok_or("the pinned Claude first-input request has no billing block")?;
    let expected = format!(
        "{PREFIX}{}{SUFFIX}",
        claude_first_input_fingerprint(authored)
    );
    if header["type"] != "text"
        || header["text"].as_str() != Some(expected.as_str())
        || system
            .iter()
            .filter(|block| {
                block["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("x-anthropic-billing-header:"))
            })
            .count()
            != 1
    {
        return Err(
            "the pinned first-input billing header, location or derived fingerprint differs",
        );
    }
    let Some(Json::String(text)) = input["system"][0].get_mut("text") else {
        return Err("the validated billing text is missing");
    };
    text.replace_range(PREFIX.len()..PREFIX.len() + 3, "<first-input fingerprint>");
    Ok(input)
}

/// Retains each synthetic witness's native role and order. Script literals in a
/// user prompt remain user content; tool calls/results are not assistant text.
/// `stop` is a uniquely authored user-input marker in these synthetic cases.
#[must_use]
pub fn marker_tape(body: &Json, markers: &[&str], stop: Option<&str>) -> Vec<(String, String)> {
    let mut tape = Vec::new();
    let rows = body
        .get("messages")
        .or_else(|| body.get("input"))
        .and_then(Json::as_array);
    for row in rows.into_iter().flatten() {
        let role = match row["type"].as_str() {
            Some("function_call" | "custom_tool_call") => "tool_call",
            Some("function_call_output" | "custom_tool_call_output") => "tool_result",
            _ => row["role"].as_str().unwrap_or("other"),
        };
        let blocks = row.get("content").and_then(Json::as_array);
        if let Some(blocks) = blocks {
            for block in blocks {
                let role = match block["type"].as_str() {
                    Some("tool_use") => "tool_call",
                    Some("tool_result") => "tool_result",
                    _ => role,
                };
                if take(block, role, markers, stop, &mut tape) {
                    return tape;
                }
            }
        } else if take(row, role, markers, stop, &mut tape) {
            return tape;
        }
    }
    tape
}

fn take(
    value: &Json,
    role: &str,
    markers: &[&str],
    stop: Option<&str>,
    tape: &mut Vec<(String, String)>,
) -> bool {
    let mut text = String::new();
    strings(value, &mut text);
    if role == "user" && stop.is_some_and(|stop| text.contains(stop)) {
        return true;
    }
    for marker in markers {
        if text.contains(marker) {
            tape.push((role.into(), (*marker).into()));
        }
    }
    false
}

fn strings(value: &Json, into: &mut String) {
    match value {
        Json::String(text) => {
            into.push_str(text);
            into.push('\n');
        }
        Json::Array(values) => {
            for value in values {
                strings(value, into);
            }
        }
        Json::Object(values) => {
            for value in values.values() {
                strings(value, into);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_input_rejects_unmarked_interruptions_unknown_blocks_and_tool_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = json!({"instructions":"every instruction stays","input":[
            {"role":"developer","content":[{"type":"input_text","text":"standing context"}]},
            {"type":"function_call","id":"item-call","call_id":"call","name":"read","arguments":"{\"path\":\"kept\"}"},
            {"type":"function_call_output","call_id":"call","output":"actual result"},
            {"role":"user","content":[{"type":"input_text","text":"SELECTED"}]}
        ]});
        let expected = full_model_input(&source, "SELECTED")?;
        let mut revised = source.clone();
        revised["input"][3]["content"][0]["text"] = json!("REVISED");
        assert_eq!(full_model_input(&revised, "REVISED")?, expected);
        for role in ["user", "developer"] {
            let mut extra = revised.clone();
            extra["input"].as_array_mut().ok_or("missing fixture input")?.insert(3,
                json!({"role":role,"content":[{"type":"input_text","text":"<turn_aborted>unmarked guidance</turn_aborted>"}]}));
            assert_ne!(full_model_input(&extra, "REVISED")?, expected);
        }
        let mut hidden = revised.clone();
        hidden["input"][0]["content"]
            .as_array_mut()
            .ok_or("missing fixture blocks")?
            .push(json!({"type":"unknown_native_block","payload":"unmarked hidden suffix"}));
        assert_ne!(full_model_input(&hidden, "REVISED")?, expected);
        for key in ["call_id", "output"] {
            let mut changed = revised.clone();
            changed["input"][2][key] = json!("changed");
            assert_ne!(full_model_input(&changed, "REVISED")?, expected);
        }
        let mut changed = revised.clone();
        changed["instructions"] = json!("other instructions");
        assert_ne!(full_model_input(&changed, "REVISED")?, expected);
        let mut opaque = revised.clone();
        opaque["previous_response_id"] = json!("unobserved-parent");
        assert!(full_model_input(&opaque, "REVISED").is_err());
        revised["input"]
            .as_array_mut()
            .ok_or("missing fixture input")?
            .push(json!({"role":"developer","content":"unknown content after the replacement"}));
        assert_ne!(full_model_input(&revised, "REVISED")?, expected);
        Ok(())
    }

    #[test]
    fn the_authored_block_stays_in_place_before_native_trailing_blocks_and_system_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = json!({"system":"unchanged instructions","messages":[
            {"role":"user","content":[{"type":"text","text":"SELECTED"},
                {"type":"text","text":"unmarked context after the authored block"}]},
            {"role":"system","content":[{"type":"text","text":"<total_tokens>15000000 tokens left</total_tokens>",
                "cache_control":{"type":"ephemeral"}}]}
        ]});
        let expected = full_model_input(&source, "SELECTED")?;
        let mut revised = source.clone();
        revised["messages"][0]["content"][0]["text"] = json!("REVISED");
        assert_eq!(full_model_input(&revised, "REVISED")?, expected);
        let mut changed = revised.clone();
        changed["messages"][1]["content"][0]["text"] = json!("different unmarked native context");
        assert_ne!(full_model_input(&changed, "REVISED")?, expected);
        let mut reordered = revised.clone();
        reordered["messages"]
            .as_array_mut()
            .ok_or("missing messages")?
            .swap(0, 1);
        assert_ne!(full_model_input(&reordered, "REVISED")?, expected);
        let mut extra = revised.clone();
        extra["messages"]
            .as_array_mut()
            .ok_or("missing messages")?
            .push(
                json!({"role":"developer","content":"an unmarked native interruption directive"}),
            );
        assert_ne!(full_model_input(&extra, "REVISED")?, expected);
        revised["messages"][0]["content"]
            .as_array_mut()
            .ok_or("missing blocks")?
            .push(json!({"type":"text","text":"REVISED"}));
        assert!(
            full_model_input(&revised, "REVISED").is_err(),
            "exactly one authored block must be identified"
        );
        Ok(())
    }

    fn codex_body() -> Json {
        json!({"instructions":"unchanged","tools":[{"type":"function","name":"read","parameters":{}}],
        "tool_choice":"auto","input":[
            {"type":"message","id":"msg_11111111-1111-5111-8111-111111111111","role":"developer",
                "content":[{"type":"input_text","text":"base instructions"}]},
            {"type":"message","id":format!("msg_{}",orch_types::new_id()),"role":"developer",
                "content":[{"type":"input_text","text":"native context"}]},
            {"type":"message","id":format!("msg_{}",orch_types::new_id()),"role":"user",
                "content":[{"type":"input_text","text":"environment context"}]},
            {"type":"message","id":format!("msg_{}",orch_types::new_id()),"role":"user",
                "content":[{"type":"input_text","text":"SELECTED"}]}
        ]})
    }

    #[test]
    fn codex_normalizes_only_the_rebuilt_base_and_new_authored_envelopes_in_retained_history()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = codex_body();
        let rows = source["input"].as_array_mut().ok_or("missing input")?;
        rows.insert(
            3,
            json!({"type":"message","id":format!("msg_{}",orch_types::new_id()),"role":"user",
            "content":[{"type":"input_text","text":"old authored input"}]}),
        );
        rows.insert(4,json!({"type":"message","id":"msg_old_assistant","role":"assistant","content":[{"type":"output_text","text":"old response"}]}));
        rows.insert(5,json!({"type":"function_call","id":"old_call_item","call_id":"old_call","name":"read","arguments":"{}"}));
        rows.insert(6,json!({"type":"function_call_output","id":"old_result_item","call_id":"old_call","output":"old bytes"}));
        let expected = codex_model_input(&source, "SELECTED", false)?;
        let mut revised = source.clone();
        revised["input"][0]["id"] = json!("msg_22222222-2222-5222-8222-222222222222");
        revised["input"][7]["id"] = json!(format!("msg_{}", orch_types::new_id()));
        revised["input"][7]["content"][0]["text"] = json!("REVISED");
        assert_eq!(codex_model_input(&revised, "REVISED", false)?, expected);
        assert_ne!(
            full_model_input(&revised, "REVISED")?,
            full_model_input(&source, "SELECTED")?,
            "the general model oracle does not discard any envelope IDs"
        );
        for index in 1..=6 {
            let mut changed = revised.clone();
            changed["input"][index]["id"] = json!(format!("msg_{}", orch_types::new_id()));
            assert_ne!(
                codex_model_input(&changed, "REVISED", false)?,
                expected,
                "retained message/tool identities stay exact"
            );
        }
        for index in [5, 6] {
            let mut changed = revised.clone();
            changed["input"][index]["call_id"] = json!("other_call");
            assert_ne!(codex_model_input(&changed, "REVISED", false)?, expected);
        }
        let mut changed = revised.clone();
        changed["input"][0]["content"][0]["text"] = json!("other base instructions");
        assert_ne!(codex_model_input(&changed, "REVISED", false)?, expected);
        let mut extra = revised.clone();
        extra["input"]
            .as_array_mut()
            .ok_or("missing input")?
            .push(json!({"role":"developer","content":"unmarked interruption after input"}));
        assert_ne!(codex_model_input(&extra, "REVISED", false)?, expected);
        revised["input"][0]["id"] = json!(format!("msg_{}", orch_types::new_id()));
        assert!(
            codex_model_input(&revised, "REVISED", false).is_err(),
            "only the pinned thread-scoped UUIDv5 prefix is normalized"
        );
        Ok(())
    }

    #[test]
    fn an_explicitly_empty_codex_prefix_regenerates_only_its_initial_context_envelopes()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = codex_body();
        let expected = codex_model_input(&source, "SELECTED", true)?;
        let mut revised = source.clone();
        revised["input"][0]["id"] = json!("msg_22222222-2222-5222-8222-222222222222");
        for index in 1..=3 {
            revised["input"][index]["id"] = json!(format!("msg_{}", orch_types::new_id()));
        }
        revised["input"][3]["content"][0]["text"] = json!("REVISED");
        assert_eq!(codex_model_input(&revised, "REVISED", true)?, expected);
        assert_ne!(
            codex_model_input(&revised, "REVISED", false)?,
            codex_model_input(&source, "SELECTED", false)?,
            "retained context IDs must not acquire the empty-prefix exemption"
        );
        let mut changed = revised.clone();
        changed["input"][2]["content"][0]["text"] = json!("different environment context");
        assert_ne!(codex_model_input(&changed, "REVISED", true)?, expected);
        for row in [
            json!({"role":"assistant","content":"old response"}),
            json!({"type":"function_call","id":"call_item","call_id":"call","arguments":"{}"}),
        ] {
            let mut hidden = revised.clone();
            hidden["input"]
                .as_array_mut()
                .ok_or("missing input")?
                .insert(3, row);
            assert!(
                codex_model_input(&hidden, "REVISED", true).is_err(),
                "old conversation/tool material is not bootstrap context"
            );
        }
        let mut hidden = revised.clone();
        hidden["input"][1]["content"]
            .as_array_mut()
            .ok_or("missing blocks")?
            .push(json!({"type":"unknown_native_block","payload":"not text context"}));
        assert!(codex_model_input(&hidden, "REVISED", true).is_err());
        revised["input"][3]["id"] = json!("old_or_missing_authored_id");
        assert!(codex_model_input(&revised, "REVISED", true).is_err());
        Ok(())
    }

    #[test]
    fn claude_first_billing_fingerprint_matches_the_pinned_utf16_sampling() {
        assert_eq!(claude_first_input_fingerprint("PREFIX-USER-A"), "fe0");
        assert_eq!(
            claude_first_input_fingerprint("PREFIX-REVISED-INPUT"),
            "2e9"
        );
        assert_eq!(
            claude_first_input_fingerprint(""),
            "406",
            "missing indices contribute the string zero"
        );
        assert_eq!(
            claude_first_input_fingerprint("0123😀abc"),
            "3c3",
            "a sampled unpaired UTF-16 surrogate is UTF-8 replacement-encoded, not indexed as a Unicode scalar"
        );
        assert_eq!(
            claude_first_input_fingerprint("0123😀a\0c"),
            "77b",
            "a present NUL code unit is not an absent index"
        );
    }

    fn first_claude_body() -> Json {
        json!({"system":[
            {"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.296.fe0; cc_entrypoint=sdk-cli;"},
            {"type":"text","text":"all unrelated native system instructions stay exact"}
        ],"tools":[{"name":"read","input_schema":{"type":"object"},
            "description":"a literal cc_version=2.1.296.fe0 is ordinary tool content"}],"tool_choice":{"type":"auto"},
            "messages":[{"role":"user","content":[{"type":"text","text":"native context before authored input"},
                {"type":"text","text":"PREFIX-USER-A"}]},
                {"role":"system","content":[{"type":"text","text":"<total_tokens>15000000 tokens left</total_tokens>"}]}]})
    }

    #[test]
    fn first_claude_input_normalizes_only_a_verified_checksum_and_retains_every_unrelated_byte()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = first_claude_body();
        let expected = claude_first_model_input(&source, "PREFIX-USER-A")?;
        let mut revised = source.clone();
        revised["messages"][0]["content"][1]["text"] = json!("PREFIX-REVISED-INPUT");
        revised["system"][0]["text"] =
            json!("x-anthropic-billing-header: cc_version=2.1.296.2e9; cc_entrypoint=sdk-cli;");
        assert_eq!(
            claude_first_model_input(&revised, "PREFIX-REVISED-INPUT")?,
            expected
        );
        assert_eq!(
            expected["system"][0]["text"],
            "x-anthropic-billing-header: cc_version=2.1.296.<first-input fingerprint>; cc_entrypoint=sdk-cli;"
        );
        assert_ne!(
            full_model_input(&source, "PREFIX-USER-A")?,
            full_model_input(&revised, "PREFIX-REVISED-INPUT")?,
            "the ordinary middle/queued oracle retains the original strict header comparison"
        );
        let mut changed = revised.clone();
        changed["system"][1]["text"] = json!("other native instructions");
        assert_ne!(
            claude_first_model_input(&changed, "PREFIX-REVISED-INPUT")?,
            expected
        );
        let mut changed = revised.clone();
        changed["system"][0]["unknown_native_metadata"] = json!("additional system data");
        assert_ne!(
            claude_first_model_input(&changed, "PREFIX-REVISED-INPUT")?,
            expected
        );
        let mut changed = revised.clone();
        changed["messages"][0]["content"][0]["text"] = json!("different native context");
        assert_ne!(
            claude_first_model_input(&changed, "PREFIX-REVISED-INPUT")?,
            expected
        );
        let mut changed = revised.clone();
        changed["messages"][1]["content"][0]["text"] =
            json!("different trailing native token context");
        assert_ne!(
            claude_first_model_input(&changed, "PREFIX-REVISED-INPUT")?,
            expected
        );
        let mut changed = revised.clone();
        changed["tools"][0]["description"] =
            json!("a literal cc_version=2.1.296.2e9 is ordinary tool content");
        assert_ne!(
            claude_first_model_input(&changed, "PREFIX-REVISED-INPUT")?,
            expected,
            "header-like content outside its exact first system text is never normalized"
        );
        revised["system"]
            .as_array_mut()
            .ok_or("missing system blocks")?
            .push(json!({"type":"text","text":"unmarked extra guidance"}));
        assert_ne!(
            claude_first_model_input(&revised, "PREFIX-REVISED-INPUT")?,
            expected
        );
        Ok(())
    }

    #[test]
    fn malformed_misplaced_or_wrongly_derived_claude_billing_headers_cannot_receive_the_first_input_exemption()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = first_claude_body();
        for text in [
            "x-anthropic-billing-header: cc_version=2.1.296.000; cc_entrypoint=sdk-cli;",
            "x-anthropic-billing-header: cc_version=2.1.296.fe0; cc_entrypoint=cli;",
            "x-anthropic-billing-header: cc_version=2.1.297.fe0; cc_entrypoint=sdk-cli;",
            "x-anthropic-billing-header: cc_version=2.1.296.fe0; cc_entrypoint=sdk-cli; new_field=data;",
            "x-anthropic-billing-header: cc_version=2.1.296.fe0; cc_entrypoint=sdk-cli;\nextra native instructions",
            "other system content",
        ] {
            let mut changed = source.clone();
            changed["system"][0]["text"] = json!(text);
            assert!(claude_first_model_input(&changed, "PREFIX-USER-A").is_err());
        }
        let mut misplaced = source.clone();
        misplaced["system"]
            .as_array_mut()
            .ok_or("missing system")?
            .swap(0, 1);
        assert!(claude_first_model_input(&misplaced, "PREFIX-USER-A").is_err());
        let mut duplicate = source.clone();
        duplicate["system"]
            .as_array_mut()
            .ok_or("missing system")?
            .push(source["system"][0].clone());
        assert!(claude_first_model_input(&duplicate, "PREFIX-USER-A").is_err());
        let mut absent = source.clone();
        absent["system"] = json!([]);
        assert!(claude_first_model_input(&absent, "PREFIX-USER-A").is_err());
        let mut middle = source.clone();
        middle["messages"]
            .as_array_mut()
            .ok_or("missing messages")?
            .insert(
                1,
                json!({"role":"user","content":[{"type":"text","text":"MIDDLE"}]}),
            );
        assert!(
            claude_first_model_input(&middle, "MIDDLE").is_err(),
            "an ordinary middle input has no exemption"
        );
        let mut changed = middle.clone();
        changed["messages"][1]["content"][0]["text"] = json!("REVISED");
        assert_eq!(
            full_model_input(&middle, "MIDDLE")?,
            full_model_input(&changed, "REVISED")?
        );
        changed["system"][0]["text"] =
            json!("x-anthropic-billing-header: cc_version=2.1.296.2e9; cc_entrypoint=sdk-cli;");
        assert_ne!(
            full_model_input(&middle, "MIDDLE")?,
            full_model_input(&changed, "REVISED")?
        );
        Ok(())
    }

    #[test]
    fn full_input_keeps_mixed_tool_results_and_only_normalizes_known_cache_hints()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = json!({"system":[{"type":"text","text":"system context","cache_control":{"type":"ephemeral"}}],
        "tools":[{"name":"run","input_schema":{"type":"object"},"cache_control":{"type":"ephemeral","ttl":"5m"}}],
        "messages":[
            {"role":"assistant","content":[{"type":"tool_use","id":"kept-call","name":"run",
                "input":{"cache_control":{"type":"ephemeral"},"command":"actual command"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"kept-call","content":[{"type":"text","text":"unmarked result"}],
                "cache_control":{"type":"ephemeral","ttl":"1h"}},{"type":"text","text":"SELECTED"}]}
        ]});
        let expected = full_model_input(&source, "SELECTED")?;
        let mut revised = source.clone();
        revised["messages"][1]["content"][1]["text"] = json!("REVISED");
        revised["system"][0]
            .as_object_mut()
            .ok_or("missing fixture system")?
            .remove("cache_control");
        revised["tools"][0]
            .as_object_mut()
            .ok_or("missing fixture tool")?
            .remove("cache_control");
        revised["messages"][1]["content"][0]
            .as_object_mut()
            .ok_or("missing fixture result")?
            .remove("cache_control");
        assert_eq!(full_model_input(&revised, "REVISED")?, expected);
        let mut changed = revised.clone();
        changed["messages"][0]["content"][0]["input"]["cache_control"] = Json::Null;
        assert_ne!(
            full_model_input(&changed, "REVISED")?,
            expected,
            "tool arguments are actual input, not cache metadata"
        );
        let mut unknown = revised.clone();
        unknown["messages"][1]["content"][0]["cache_control"] =
            json!({"type":"ephemeral","unknown":"unmarked data"});
        assert_ne!(
            full_model_input(&unknown, "REVISED")?,
            expected,
            "unknown cache-like data cannot disappear"
        );
        let mut hidden = revised;
        hidden["messages"][1]["content"][0]["content"][0]["text"] = json!("other result");
        assert_ne!(full_model_input(&hidden, "REVISED")?, expected);
        Ok(())
    }

    #[test]
    fn failure_evidence_redacts_the_known_synthetic_credential_and_respects_the_native_frame_ceiling()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes =
            b"{\"native_identity\":\"retained-input\",\"synthetic_auth\":\"fixture-credential\"}\n";
        let kept = evidence_bytes(
            bytes,
            &["fixture-credential"],
            bytes.len() + "<synthetic credential omitted>".len(),
        )?;
        let row: Json = serde_json::from_slice(&kept)?;
        assert_eq!(row["native_identity"], "retained-input");
        assert_eq!(row["synthetic_auth"], "<synthetic credential omitted>");
        assert!(evidence_bytes(bytes, &[], bytes.len() - 1).is_err());
        assert!(evidence_bytes(bytes, &["fixture-credential"], bytes.len()).is_err());
        Ok(())
    }

    #[test]
    fn source_preservation_allows_only_exact_old_bytes_and_known_administrative_appends()
    -> Result<(), Box<dyn std::error::Error>> {
        let old=b"{\"type\":\"user\",\"uuid\":\"original-input\",\"message\":{\"content\":\"original words\"}}\n{\"type\":\"assistant\",\"uuid\":\"original-answer\",\"message\":{\"content\":\"original answer\"}}\n";
        let admin = json!({"type":"last-prompt","sessionId":"source","leafUuid":"original-answer","lastPrompt":"original words"});
        let mut good = old.to_vec();
        good.extend_from_slice(format!("{admin}\n").as_bytes());
        assert_eq!(claude_source_preserved(old, &good, "source")?, 1);
        assert!(claude_source_preserved(old, &good, "another-source").is_err());
        let mut changed = good.clone();
        changed[0] = b' ';
        assert!(claude_source_preserved(old, &changed, "source").is_err());
        let mode = json!({"type":"mode","sessionId":"source","mode":"normal"});
        let mut closed = good.clone();
        closed.extend_from_slice(format!("{mode}\n").as_bytes());
        assert_eq!(claude_source_preserved(old, &closed, "source")?, 2);
        let mut carrying = mode;
        carrying["message"] = json!({"content":"other words"});
        let mut added = old.to_vec();
        added.extend_from_slice(format!("{carrying}\n").as_bytes());
        assert!(claude_source_preserved(old, &added, "source").is_err());
        let mut added = old.to_vec();
        added.extend_from_slice(
            b"{\"type\":\"assistant\",\"sessionId\":\"source\",\"uuid\":\"other\"}\n",
        );
        assert!(claude_source_preserved(old, &added, "source").is_err());
        let mut hidden = admin;
        hidden["message"] = json!({"content":"other words"});
        let mut added = old.to_vec();
        added.extend_from_slice(format!("{hidden}\n").as_bytes());
        assert!(claude_source_preserved(old, &added, "source").is_err());
        Ok(())
    }

    #[test]
    fn container_mode_is_explicit_marked_and_limited_to_the_prefix_target() {
        assert_eq!(
            container_mode(None, "another_target::common", false),
            Ok(false)
        );
        assert!(container_mode(Some(OsStr::new("0")), "input_prefix::common", true).is_err());
        assert!(container_mode(Some(OsStr::new("1")), "input_prefix::common", false).is_err());
        assert!(container_mode(Some(OsStr::new("1")), "sessions::common", true).is_err());
        assert_eq!(
            container_mode(Some(OsStr::new("1")), "input_prefix::common", true),
            Ok(true)
        );
    }

    #[test]
    fn only_the_native_cli_or_owned_unchanged_wire_gate_can_use_direct_proof_launching()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let roots = [home.path().to_owned()];
        let gate = home.path().join("prefix-wire-gate.cjs");
        std::fs::write(&gate, STEER_WIRE_GATE)?;
        let args = [
            gate.to_string_lossy().into_owned(),
            "codex".into(),
            "app-server".into(),
        ];
        assert_eq!(
            prefix_native_args_offset("codex", &["app-server".into()], "codex", &roots),
            Ok(0)
        );
        assert_eq!(
            prefix_native_args_offset("node", &args, "codex", &roots),
            Ok(2)
        );
        assert!(prefix_native_args_offset("sh", &args, "codex", &roots).is_err());
        assert!(prefix_native_args_offset("node", &args, "claude", &roots).is_err());
        assert!(prefix_native_args_offset("node", &args, "codex", &[]).is_err());
        std::fs::write(&gate, "a different launcher")?;
        assert!(prefix_native_args_offset("node", &args, "codex", &roots).is_err());
        Ok(())
    }

    #[test]
    fn claude_mixed_tool_result_and_steer_keep_the_actual_native_block_boundary() {
        let body = json!({"messages":[
            {"role":"user","content":"script mentions BEFORE and AFTER"},
            {"role":"assistant","content":[{"type":"text","text":"BEFORE"},{"type":"tool_use","input":{"command":"HELD"}}]},
            {"role":"user","content":[{"type":"tool_result","content":"RESULT"},{"type":"text","text":"STEER"}]},
            {"role":"assistant","content":"AFTER"}
        ]});
        assert_eq!(
            marker_tape(
                &body,
                &["BEFORE", "AFTER", "HELD", "RESULT", "STEER"],
                Some("STEER")
            ),
            vec![
                ("user".into(), "BEFORE".into()),
                ("user".into(), "AFTER".into()),
                ("assistant".into(), "BEFORE".into()),
                ("tool_call".into(), "HELD".into()),
                ("tool_result".into(), "RESULT".into())
            ]
        );
    }

    #[test]
    fn responses_tool_rows_are_included_without_coarsening_a_steer_to_its_turn() {
        let body = json!({"input":[{"role":"user","content":[{"text":"ORIGINAL"}]},
            {"type":"function_call","arguments":"HELD"},{"role":"user","content":[{"text":"STEER"}]},
            {"type":"function_call_output","output":"RESULT"}]});
        assert_eq!(
            marker_tape(
                &body,
                &["ORIGINAL", "HELD", "STEER", "RESULT"],
                Some("STEER")
            ),
            vec![
                ("user".into(), "ORIGINAL".into()),
                ("tool_call".into(), "HELD".into())
            ]
        );
    }
}
