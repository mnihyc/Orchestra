//! A strict synthetic native-prefix program for orchestration proofs over the real link/journal.
//! Vendor model-history exactness is proved separately by the pinned adapter targets.

use orch_agent_contract::{
    descriptor::AgentDescriptor,
    history::{NativeInputPrefix, NativePrefixOpened, NativePrefixSource},
};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode, SessionOpenResult,
};
use orch_testkit::doubles::machine::ScriptedHandler;
use orch_types::Id;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};

pub fn install(
    handler: &ScriptedHandler,
    agent: Id,
    provider: Id,
    original: NativePrefixSource,
    turn: Id,
    descriptor: AgentDescriptor,
) {
    let thread = original.thread_id;
    handler.on(CommandKind::AgentStart, move |command| {
        let Command::AgentStart(body) = command else {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "wrong native command",
            ));
        };
        if body.agent_id != agent || body.provider_id != provider {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "another synthetic program was requested",
            ));
        }
        Ok(CommandResult::AgentStart(
            orch_link_contract::commands::AgentStartResult {
                reused: true,
                harness_version: "1.0.0".into(),
                descriptor: descriptor.clone(),
            },
        ))
    });
    let current = Arc::new(Mutex::new(Some(original.clone())));
    let known = Arc::new(Mutex::new(HashSet::from([original
        .native_session_ref
        .clone()])));
    let prefix = NativeInputPrefix {
        harness_kind: "fake".into(),
        source: original.clone(),
        before_input_ref: turn.to_string(),
        fingerprint: orch_types::Hash::of(b"synthetic scoped native checkpoint"),
        empty: true,
    };
    let expected = prefix.clone();
    let slot = current.clone();
    handler.on(CommandKind::SessionPrefixInspect, move |command| {
        let Command::SessionPrefixInspect(body) = command else {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "wrong native command",
            ));
        };
        if body.source != expected.source
            || body.before_input_ref != expected.before_input_ref
            || slot.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
                != Some(&expected.source)
        {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "The exact native source is not idle here.",
            ));
        }
        Ok(CommandResult::SessionPrefixInspect(expected.clone()))
    });
    let slot = current.clone();
    let registry = known.clone();
    handler.on(CommandKind::SessionPrefixOpen, move |command| {
        let Command::SessionPrefixOpen(body) = command else {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "wrong native command",
            ));
        };
        let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if body.prefix != prefix || slot.is_some() {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "Another native lane occupies this thread.",
            ));
        }
        let child = format!("scoped-native-child-{}", body.session.segment_id);
        registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(child.clone());
        *slot = Some(NativePrefixSource {
            thread_id: thread,
            segment_id: body.session.segment_id,
            native_session_ref: child.clone(),
        });
        Ok(CommandResult::SessionPrefixOpen(NativePrefixOpened {
            prefix: body.prefix.clone(),
            session: orch_agent_contract::session::SessionOpened {
                native_session_ref: child,
                resumed: false,
                import_report: None,
                instruction_sources: Vec::new(),
                approval_reviewer: None,
            },
        }))
    });
    let slot = current.clone();
    handler.on(CommandKind::SessionContinuationClose, move |command| {
        let Command::SessionContinuationClose(body) = command else {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "wrong native command",
            ));
        };
        let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|held| held != &body.source) {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "The close names another native lane.",
            ));
        }
        *slot = None;
        Ok(CommandResult::SessionContinuationClose(Empty {}))
    });
    let refresh_current = current.clone();
    let refresh_known = known.clone();
    handler.on(CommandKind::SessionContinuationResume, move |command| {
        let Command::SessionContinuationResume(body) = command else {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "wrong native command",
            ));
        };
        let mut slot = current.lock().unwrap_or_else(PoisonError::into_inner);
        if !known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&body.source.native_session_ref)
            || slot.as_ref().is_some_and(|held| {
                held != &body.source
                    && body.replace_segment_id != Some(held.segment_id)
                    && body.replace_retained_source.as_ref() != Some(held)
            })
        {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "The exact retained native lane is unavailable.",
            ));
        }
        *slot = Some(NativePrefixSource {
            thread_id: thread,
            segment_id: body.session.segment_id,
            native_session_ref: body.source.native_session_ref.clone(),
        });
        Ok(CommandResult::SessionContinuationResume(
            SessionOpenResult {
                native_session_ref: body.source.native_session_ref.clone(),
                resumed: true,
                import_report: None,
                instruction_sources: Vec::new(),
                approval_reviewer: None,
            },
        ))
    });
    let current = refresh_current;
    let known = refresh_known;
    handler.on(CommandKind::SessionOpenFenced, move |command| {
        let Command::SessionOpenFenced(body) = command else {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "wrong native command",
            ));
        };
        let held = current.lock().unwrap_or_else(PoisonError::into_inner);
        let held = held.as_ref().ok_or_else(|| {
            CommandError::new(ErrorCode::Busy, "No synthetic native lane is prepared.")
        })?;
        if body.thread_id != thread
            || body.session.agent_id != agent
            || body.session.segment_id != held.segment_id
            || body.session.native_session_ref.as_deref() != Some(held.native_session_ref.as_str())
            || body.predecessor.as_ref() != Some(held)
            || !known
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&held.native_session_ref)
        {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "The synthetic session refresh names another prepared lane.",
            ));
        }
        Ok(CommandResult::SessionOpenFenced(SessionOpenResult {
            native_session_ref: held.native_session_ref.clone(),
            resumed: true,
            import_report: None,
            instruction_sources: Vec::new(),
            approval_reviewer: None,
        }))
    });
}
