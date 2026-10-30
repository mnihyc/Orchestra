//! Exact user-input checkpoints in the pinned native transcript and truncating fork.

use super::*;
use orch_agent_contract::history::{NativeInputPrefix, NativePrefixOpened, NativePrefixSource};
use orch_agent_contract::review::ApprovalReviewer;
use orch_types::Hash;
use std::collections::HashSet;
use std::io::{BufRead, Read};
use std::path::Path;

struct Checkpoint {
    prefix: NativeInputPrefix,
    through: Option<String>,
}

fn refused(words: &str) -> AdapterError {
    AdapterError::Other {
        code: "invalid_input".into(),
        message: words.into(),
        retryable: false,
    }
}

fn user_prompt(row: &Json) -> bool {
    row["type"] == "user"
        && row["isSidechain"] != true
        && row["isMeta"] != true
        && row.get("teamName").is_none_or(Json::is_null)
        && (row["message"]["content"].is_string()
            || row["message"]["content"].as_array().is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    matches!(block["type"].as_str(), Some("text" | "image" | "document"))
                })
            }))
}

fn checkpoint(
    path: &Path,
    source: NativePrefixSource,
    before: &str,
) -> Result<Checkpoint, AdapterError> {
    if before.parse::<orch_types::Id>().is_err() {
        return Err(refused(
            "The native user-input identity is not a transcript UUID.",
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|_| refused("The native source transcript cannot be read."))?;
    let mut input = std::io::BufReader::new(file);
    let mut line = Vec::new();
    let mut parents = HashMap::<String, Option<String>>::new();
    let mut ids = HashSet::new();
    let mut hasher = Hash::hasher();
    let mut users = 0u64;
    let mut found = None;
    loop {
        line.clear();
        let read = input
            .by_ref()
            .take(crate::config::FRAME_MAX_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| refused("The native source transcript cannot be read."))?;
        if read == 0 {
            break;
        }
        if line.len() > crate::config::FRAME_MAX_BYTES {
            return Err(refused(
                "A native transcript entry exceeds the harness frame limit.",
            ));
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            if found.is_none() {
                hasher.update(&line);
            }
            continue;
        }
        let row: Json = serde_json::from_slice(&line)
            .map_err(|_| refused("The native transcript has an incomplete or invalid entry."))?;
        if let Some(version) = row.get("version").and_then(Json::as_str)
            && version != "2.1.296"
        {
            return Err(AdapterError::Unsupported(
                "The source transcript was written by a different Claude Code version.".into(),
            ));
        }
        if row["type"] == "attachment"
            && row["attachment"]["type"] == "queued_command"
            && (row["attachment"]["source_uuid"].as_str() == Some(before)
                || row["uuid"].as_str() == Some(before))
        {
            return Err(AdapterError::Unsupported(
                "That input is retained as native mid-turn system guidance, not a user-message checkpoint.".into()));
        }
        if matches!(
            row["type"].as_str(),
            Some("user" | "assistant" | "progress" | "system" | "attachment")
        ) && let Some(id) = row.get("uuid").and_then(Json::as_str)
        {
            if !ids.insert(id.to_owned()) {
                return Err(refused(
                    "The native transcript contains an ambiguous message identity.",
                ));
            }
            let parent = match row.get("parentUuid") {
                None | Some(Json::Null) => None,
                Some(Json::String(parent)) => Some(parent.clone()),
                _ => {
                    return Err(refused(
                        "The native transcript has an invalid parent checkpoint.",
                    ));
                }
            };
            if id == before {
                if !user_prompt(&row) {
                    return Err(refused(
                        "That native identity does not name a main-conversation user input.",
                    ));
                }
                found = Some((parent, Hash::finish(&hasher), users == 0));
            } else if found.is_none() {
                parents.insert(id.to_owned(), parent);
            }
        }
        if found.is_none() {
            users += u64::from(user_prompt(&row));
            hasher.update(&line);
        }
    }
    let (through, fingerprint, empty) = found
        .ok_or_else(|| refused("That native user input is not retained in this source session."))?;
    let mut next = through.as_deref();
    let mut chain = HashSet::new();
    while let Some(id) = next {
        if id == before || !chain.insert(id) {
            return Err(refused(
                "The native transcript has a cyclic parent checkpoint.",
            ));
        }
        next = parents
            .get(id)
            .ok_or_else(|| refused("The native input's parent checkpoint is missing."))?
            .as_deref();
    }
    if !empty && through.is_none() {
        return Err(refused(
            "The native input does not retain its earlier conversation checkpoint.",
        ));
    }
    Ok(Checkpoint {
        prefix: NativeInputPrefix {
            harness_kind: crate::config::KIND.into(),
            source,
            before_input_ref: before.into(),
            fingerprint,
            empty,
        },
        through,
    })
}

impl ClaudeCodeAdapter {
    async fn check_prefix_source(
        &mut self,
        source: &NativePrefixSource,
    ) -> Result<(), AdapterError> {
        if self.started()?.version != "2.1.296" {
            return Err(AdapterError::Unsupported(
                "Exact input-prefix preparation requires Claude Code 2.1.296.".into(),
            ));
        }
        if let Some(session) = self.sessions.get(&source.thread_id) {
            if session.native_ref != source.native_session_ref
                || session.spec.settings.segment_id != source.segment_id
            {
                return Err(refused(
                    "The source lane now names another native session or segment.",
                ));
            }
            if !session
                .handle
                .ask(|reply| Command::Quiescent { reply })
                .await?
            {
                return Err(refused(
                    "The source session still has a running turn or pending steer. Stop it and wait for its native end.",
                ));
            }
        } else if !self.prefix_sources.contains(source) {
            return Err(AdapterError::Other {code:"session.source_unobserved".into(),retryable:true,
                message:"This exact source lane has no current idle or idle-close observation in this harness process.".into()});
        }
        Ok(())
    }

    fn read_prefix(
        &self,
        source: NativePrefixSource,
        before: &str,
    ) -> Result<Checkpoint, AdapterError> {
        let home = self.started()?.setup.home.as_ref().ok_or_else(|| {
            AdapterError::Unsupported("This harness has no retained transcript home.".into())
        })?;
        let path = history::find_transcript(home, &source.native_session_ref).ok_or_else(|| {
            refused("The native source session is not retained in this harness home.")
        })?;
        checkpoint(&path, source, before)
    }

    pub(super) async fn inspect_prefix(
        &mut self,
        source: NativePrefixSource,
        before: &str,
    ) -> Result<NativeInputPrefix, AdapterError> {
        self.check_prefix_source(&source).await?;
        Ok(self.read_prefix(source, before)?.prefix)
    }

    pub(super) async fn open_prefix(
        &mut self,
        mut spec: SessionSpec,
        prefix: NativeInputPrefix,
    ) -> Result<NativePrefixOpened, AdapterError> {
        if prefix.harness_kind != crate::config::KIND
            || spec.settings.mode != SessionMode::Fork
            || spec.settings.thread_id != prefix.source.thread_id
            || spec.settings.segment_id == prefix.source.segment_id
            || spec.settings.native_session_ref.as_deref()
                != Some(prefix.source.native_session_ref.as_str())
            || spec.settings.upto_turn_ordinal.is_some()
        {
            return Err(refused(
                "An input-prefix open names its exact native source and no ordinal cut.",
            ));
        }
        if self.sessions.contains_key(&spec.settings.thread_id) {
            return Err(refused(
                "The target session lane is occupied. Close it before opening the prepared prefix.",
            ));
        }
        let reviewer = self.validate_open(&spec)?;
        self.check_prefix_source(&prefix.source).await?;
        let current = self.read_prefix(prefix.source.clone(), &prefix.before_input_ref)?;
        if current.prefix != prefix {
            return Err(refused(
                "The retained native prefix changed after it was inspected.",
            ));
        }
        let origin = if prefix.empty {
            spec.settings.mode = SessionMode::New;
            spec.settings.native_session_ref = None;
            Origin::New
        } else {
            Origin::ForkPrefix {
                from: prefix.source.native_session_ref.clone(),
                through: current
                    .through
                    .ok_or_else(|| refused("The native prefix has no parent checkpoint."))?,
            }
        };
        let opened = self
            .install_session(
                spec,
                orch_types::new_id().to_string(),
                origin,
                false,
                reviewer,
            )
            .await?;
        if opened.native_session_ref == prefix.source.native_session_ref {
            return Err(refused(
                "The harness reused its source identity instead of creating a fresh prefix session.",
            ));
        }
        Ok(NativePrefixOpened {
            session: opened,
            prefix,
        })
    }

    pub(super) fn validate_open(
        &self,
        spec: &SessionSpec,
    ) -> Result<Option<ApprovalReviewer>, AdapterError> {
        if spec.settings.sandbox_mode == orch_agent_contract::descriptor::SandboxMode::ReadOnly {
            return Err(AdapterError::Unsupported(
                "Read-only is not one of this Claude Code adapter's sandbox modes.".into(),
            ));
        }
        if let Some(reading) = spec.settings.side_reading {
            return Err(AdapterError::Unsupported(format!(
                "Claude Code has no declared side-conversation mode for {reading:?}."
            )));
        }
        crate::sessions::validate_instruction_settings(spec)?;
        if spec.settings.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Auto
            && !crate::descriptor::auto_proven()
        {
            return Err(AdapterError::Unsupported(
                "Auto approval is not offered by this Claude Code harness.".into(),
            ));
        }
        crate::review::reviewer(&spec.settings)
    }

    pub(super) async fn install_session(
        &mut self,
        spec: SessionSpec,
        session_id: String,
        origin: Origin,
        resumed: bool,
        approval_reviewer: Option<ApprovalReviewer>,
    ) -> Result<SessionOpened, AdapterError> {
        let started = self.started()?.clone();
        let thread = spec.settings.thread_id;
        crate::sessions::skills_plugin(&spec)?;
        if let Some(old) = self.sessions.remove(&thread) {
            let _ = old.handle.ask(|reply| Command::Close { reply }).await;
        }
        let fork = matches!(origin, Origin::Fork { .. } | Origin::ForkPrefix { .. });
        let handle = conversation::start(Params {
            thread_id: thread,
            segment_id: spec.settings.segment_id,
            spec: spec.clone(),
            setup: Arc::clone(&started.setup),
            launcher: Arc::clone(&self.launcher),
            sink: started.sink.clone(),
            clock: Arc::clone(&self.clock),
            session_id: session_id.clone(),
            origin,
            token: Arc::clone(&self.token),
        });
        if fork
            || spec.settings.approval_mode == orch_agent_contract::descriptor::ApprovalMode::Auto
        {
            match handle
                .ask(|reply| Command::Warm {
                    turn: Box::new(Self::turn_settings(&spec)),
                    reply,
                })
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) | Err(error) => {
                    let _ = handle.ask(|reply| Command::Close { reply }).await;
                    return Err(error);
                }
            }
        }
        let segment = spec.settings.segment_id;
        let mut sources = crate::sessions::documented_instruction_sources(&started.setup, &spec);
        if let Ok(reported) = handle
            .ask(|reply| Command::InstructionSources { reply })
            .await
        {
            sources = crate::instruction_sources::merge(sources, reported);
        }
        self.prefix_sources
            .retain(|source| source.thread_id != thread && source.native_session_ref != session_id);
        self.sessions.insert(
            thread,
            Session {
                handle,
                native_ref: session_id.clone(),
                spec,
            },
        );
        let _ = started
            .sink
            .session(
                thread,
                segment,
                SessionEvent::SessionOpened(SessionOpenedEvent {
                    native_session_ref: session_id.clone(),
                    resumed,
                }),
            )
            .await;
        self.status_event(HarnessState::Online, None).await;
        Ok(SessionOpened {
            native_session_ref: session_id,
            resumed,
            import_report: None,
            instruction_sources: sources,
            approval_reviewer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const SESSION: &str = "11111111-1111-4111-8111-111111111111";
    const FIRST: &str = "22222222-2222-4222-8222-222222222222";
    const RESPONSE: &str = "33333333-3333-4333-8333-333333333333";
    const SECOND: &str = "44444444-4444-4444-8444-444444444444";

    fn source() -> NativePrefixSource {
        NativePrefixSource {
            thread_id: orch_types::new_id(),
            segment_id: orch_types::new_id(),
            native_session_ref: SESSION.into(),
        }
    }
    fn transcript(rows: &[Json]) -> std::io::Result<tempfile::NamedTempFile> {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new()?;
        for row in rows {
            writeln!(file, "{row}")?;
        }
        Ok(file)
    }
    fn rows() -> Vec<Json> {
        vec![
            json!({"type":"user","uuid":FIRST,"parentUuid":null,"version":"2.1.296","message":{"role":"user","content":"first"}}),
            json!({"type":"assistant","uuid":RESPONSE,"parentUuid":FIRST,"message":{"role":"assistant","content":[]}}),
            json!({"type":"user","uuid":SECOND,"parentUuid":RESPONSE,"message":{"role":"user","content":"second"}}),
        ]
    }

    #[test]
    fn the_cut_is_before_the_real_user_uuid_and_appended_suffix_does_not_change_its_fingerprint()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut rows = rows();
        let source = source();
        let file = transcript(&rows)?;
        let cut = checkpoint(file.path(), source.clone(), SECOND)?;
        assert_eq!(cut.through.as_deref(), Some(RESPONSE));
        assert!(!cut.prefix.empty);
        rows.push(json!({"type":"assistant","uuid":"55555555-5555-4555-8555-555555555555","parentUuid":SECOND}));
        let appended = transcript(&rows)?;
        assert_eq!(
            checkpoint(appended.path(), source.clone(), SECOND)?.prefix,
            cut.prefix
        );
        rows[0]["message"]["content"] = json!("changed earlier native content");
        let changed = transcript(&rows)?;
        assert_ne!(
            checkpoint(changed.path(), source, SECOND)?.prefix,
            cut.prefix
        );
        Ok(())
    }

    #[test]
    fn missing_ambiguous_non_prompt_and_cyclic_native_ids_refuse()
    -> Result<(), Box<dyn std::error::Error>> {
        let valid = rows();
        let file = transcript(&valid)?;
        assert!(
            checkpoint(
                file.path(),
                source(),
                "66666666-6666-4666-8666-666666666666"
            )
            .is_err()
        );
        assert!(checkpoint(file.path(), source(), RESPONSE).is_err());
        let mut duplicate = valid.clone();
        duplicate.push(valid[2].clone());
        assert!(checkpoint(transcript(&duplicate)?.path(), source(), SECOND).is_err());
        let mut cyclic = valid;
        cyclic[0]["parentUuid"] = json!(RESPONSE);
        assert!(checkpoint(transcript(&cyclic)?.path(), source(), SECOND).is_err());
        Ok(())
    }

    #[test]
    fn a_first_user_input_has_an_empty_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let file = transcript(&rows())?;
        assert!(checkpoint(file.path(), source(), FIRST)?.prefix.empty);
        Ok(())
    }

    #[test]
    fn queued_inputs_refuse_both_native_identities_without_rounding_to_a_user_turn()
    -> Result<(), Box<dyn std::error::Error>> {
        const QUEUED: &str = "66666666-6666-4666-8666-666666666666";
        const ATTACHMENT: &str = "77777777-7777-4777-8777-777777777777";
        let mut native = rows();
        native.push(json!({"type":"attachment","uuid":ATTACHMENT,"parentUuid":RESPONSE,
            "sessionId":SESSION,"version":"2.1.296","renderedRole":"system",
            "attachment":{"type":"queued_command","source_uuid":QUEUED,"prompt":[{"type":"text","text":"queued input"}]}}));
        let file = transcript(&native)?;
        for identity in [QUEUED, ATTACHMENT] {
            assert!(matches!(
                checkpoint(file.path(), source(), identity),
                Err(AdapterError::Unsupported(_))
            ));
        }
        let ordinary = checkpoint(file.path(), source(), SECOND)?;
        assert_eq!(ordinary.through.as_deref(), Some(RESPONSE));
        Ok(())
    }
}
