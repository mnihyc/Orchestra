//! Accepted input origins stay independent until each is admitted for the same current work.
//!
//! This module stores credential boundaries and restores their current restrictions. It does
//! not infer an operation, resolve a target, or combine distinct origins into one caller.

use crate::error::EngineError;
use orch_authz::{Caller, CapturedCaller, CapturedOperation, TokenGrant};
use orch_db::Conn;
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// Immutable accepted boundaries. Old credential records remain readable without inventing
/// operation provenance that the old producer did not retain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputAuthority {
    // Read compatibility only: new captures use the complete shared caller snapshot below.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    credentials: Vec<CredentialAuthority>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    boundaries: Vec<CapturedCaller>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    origins: Vec<CapturedOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scheduled: Option<super::ScheduledInputOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chat: Option<super::ChatInputOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forward: Option<super::ForwardInputOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot_comment: Option<super::SnapshotCommentOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum CredentialAuthority {
    Session {
        #[serde(default)]
        as_admin: bool,
    },
    Token {
        token_id: Option<Id>,
        scopes: Vec<String>,
        node_id: Option<Id>,
    },
}

/// Fresh, unadmitted boundaries for one acting account. The materialized producer must
/// admit every origin separately and apply its ordinary authorization to each caller.
#[derive(Debug, Clone)]
pub(super) struct RestoredInputAuthority {
    pub(super) actor: Id,
    pub(super) captured: InputAuthority,
    pub(super) origins: Vec<RestoredInputOrigin>,
}

#[derive(Debug, Clone)]
pub(super) struct RestoredInputOrigin {
    pub(super) operation: Option<String>,
    pub(super) caller: Caller,
}

/// A transaction-local authority bundle for one materialized input. Every contained caller has
/// passed its own exact operation, and still has to pass the ordinary producer's authorization.
/// There is deliberately no conversion to one representative caller.
#[derive(Debug, Clone)]
pub(crate) struct InputCallers {
    actor: Id,
    captured: InputAuthority,
    callers: Vec<Caller>,
}

impl InputCallers {
    pub(super) fn admitted(
        restored: RestoredInputAuthority,
        callers: Vec<Caller>,
    ) -> Result<Self, EngineError> {
        if callers.is_empty()
            || callers.len() != restored.origins.len()
            || callers.iter().any(|caller| caller.user() != restored.actor)
        {
            return Err(EngineError::unreadable(
                "input authority origins are incomplete",
            ));
        }
        Ok(Self {
            actor: restored.actor,
            captured: restored.captured,
            callers,
        })
    }

    /// Existing authorless work uses its actual ordinary owner, as before.
    pub(super) fn single(caller: Caller) -> Result<Self, EngineError> {
        Ok(Self {
            actor: caller.user(),
            captured: InputAuthority::from_caller(&caller)?,
            callers: vec![caller],
        })
    }

    /// Retain the complete current caller supplied by the ordinary guarded forward producer.
    pub(super) fn accepted_forward(
        caller: Caller,
        reference: super::ForwardInputOrigin,
    ) -> Result<Self, EngineError> {
        let captured = InputAuthority::for_forward(&caller, reference)?;
        Ok(Self {
            actor: caller.user(),
            captured,
            callers: vec![caller],
        })
    }

    pub(super) fn accepted_snapshot_comment(
        caller: Caller,
        reference: super::SnapshotCommentOrigin,
    ) -> Result<Self, EngineError> {
        let captured = InputAuthority::for_snapshot_comment(&caller, reference)?;
        Ok(Self {
            actor: caller.user(),
            captured,
            callers: vec![caller],
        })
    }

    pub(crate) fn user(&self) -> Id {
        self.actor
    }

    /// Ordinary checks must cover every origin; no one caller stands for the others.
    pub(crate) fn callers(&self) -> &[Caller] {
        &self.callers
    }

    /// Derived work retains the immutable origin bundle and rebinds it at its next checkpoint.
    pub(crate) fn captured(&self) -> &InputAuthority {
        &self.captured
    }
}

impl InputAuthority {
    fn from_caller(caller: &Caller) -> Result<Self, EngineError> {
        if caller.admitted_operation().is_some() {
            return Ok(Self {
                credentials: Vec::new(),
                boundaries: Vec::new(),
                origins: vec![CapturedOperation::capture(caller).map_err(EngineError::unreadable)?],
                scheduled: None,
                chat: None,
                forward: None,
                snapshot_comment: None,
            });
        }
        if caller.admitted_facets().is_some()
            || caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
        {
            return Err(EngineError::unreadable(
                "input authority requires one exact admitted operation",
            ));
        }
        Ok(Self {
            credentials: Vec::new(),
            boundaries: vec![CapturedCaller::capture(caller)],
            origins: Vec::new(),
            scheduled: None,
            chat: None,
            forward: None,
            snapshot_comment: None,
        })
    }

    fn validate(&self) -> Result<(), EngineError> {
        if self.credentials.is_empty() && self.boundaries.is_empty() && self.origins.is_empty() {
            return Err(EngineError::unreadable(
                "input authority has no credential boundary",
            ));
        }
        if usize::from(self.scheduled.is_some())
            + usize::from(self.chat.is_some())
            + usize::from(self.forward.is_some())
            + usize::from(self.snapshot_comment.is_some())
            > 1
        {
            return Err(EngineError::unreadable(
                "an input has conflicting producer identities",
            ));
        }
        Ok(())
    }

    pub(super) fn scheduled(&self) -> Option<super::ScheduledInputOrigin> {
        self.scheduled
    }

    pub(super) fn operation_origins(&self) -> &[CapturedOperation] {
        &self.origins
    }

    pub(crate) fn chat(&self) -> Option<super::ChatInputOrigin> {
        self.chat
    }

    pub(super) fn for_chat(
        caller: &Caller,
        reference: super::ChatInputOrigin,
    ) -> Result<Self, EngineError> {
        if caller
            .admitted_operation()
            .is_some_and(|operation| operation != "ChatMessageCreate")
        {
            return Err(EngineError::unreadable(
                "an agent ask retains its actual chat-message operation",
            ));
        }
        let mut result = Self::from_caller(caller)?;
        result.chat = Some(reference);
        Ok(result)
    }

    pub(crate) fn forward(&self) -> Option<&super::ForwardInputOrigin> {
        self.forward.as_ref()
    }

    pub(super) fn snapshot_comment(&self) -> Option<super::SnapshotCommentOrigin> {
        self.snapshot_comment
    }

    fn for_snapshot_comment(
        caller: &Caller,
        reference: super::SnapshotCommentOrigin,
    ) -> Result<Self, EngineError> {
        if caller
            .admitted_operation()
            .is_some_and(|operation| operation != "ShareAgentCreate.snapshot_comment")
        {
            return Err(EngineError::unreadable(
                "a snapshot comment retains its actual share operation",
            ));
        }
        let mut result = Self::from_caller(caller)?;
        result.snapshot_comment = Some(reference);
        Ok(result)
    }

    pub(super) fn has_snapshot_comment_origin(&self, actor: Id) -> bool {
        self.origins.iter().any(|origin| {
            origin.user() == actor && origin.operation() == "ShareAgentCreate.snapshot_comment"
        })
    }

    pub(super) fn has_forward_origin(&self, actor: Id) -> bool {
        self.origins
            .iter()
            .any(|origin| origin.user() == actor && origin.operation() == "ForwardCreate")
    }

    pub(super) fn for_forward(
        caller: &Caller,
        reference: super::ForwardInputOrigin,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some("ForwardCreate") {
            return Err(EngineError::unreadable(
                "a forward retains its actual ForwardCreate operation",
            ));
        }
        let mut result = Self::from_caller(caller)?;
        result.forward = Some(reference);
        Ok(result)
    }

    /// A job keeps its accepted ceiling and also every later accepted edit of the same input.
    /// Distinct operations remain independent; neither snapshot replaces the other.
    pub(crate) fn retaining_current(&self, actor: Id, current: &Self) -> Result<Self, EngineError> {
        self.validate_actor(actor)?;
        current.validate_actor(actor)?;
        if self.scheduled != current.scheduled
            || self.chat != current.chat
            || self.forward != current.forward
            || self.snapshot_comment != current.snapshot_comment
        {
            return Err(EngineError::unreadable(
                "the input's producer reference changed",
            ));
        }
        let mut result = self.clone();
        for credential in &current.credentials {
            if !result.credentials.contains(credential) {
                result.credentials.push(credential.clone());
            }
        }
        for boundary in &current.boundaries {
            if !result.boundaries.contains(boundary) {
                result.boundaries.push(boundary.clone());
            }
        }
        for origin in &current.origins {
            if !result.origins.contains(origin) {
                result.origins.push(origin.clone());
            }
        }
        Ok(result)
    }

    pub(super) fn for_schedule(
        actor: Id,
        origins: &[CapturedOperation],
        reference: super::ScheduledInputOrigin,
    ) -> Result<Self, EngineError> {
        if origins.iter().any(|origin| {
            !matches!(
                origin.operation(),
                "TopicInputScheduleCreate" | "InputScheduleUpdate.definition"
            )
        }) {
            return Err(EngineError::unreadable(
                "a scheduled input needs its actual author-definition operations",
            ));
        }
        let result = Self {
            credentials: Vec::new(),
            boundaries: Vec::new(),
            origins: origins.to_vec(),
            scheduled: Some(reference),
            chat: None,
            forward: None,
            snapshot_comment: None,
        };
        result.validate_actor(actor)?;
        Ok(result)
    }

    fn validate_actor(&self, actor: Id) -> Result<(), EngineError> {
        self.validate()?;
        if self.origins.iter().any(|origin| origin.user() != actor)
            || self
                .boundaries
                .iter()
                .any(|boundary| boundary.user() != actor)
        {
            return Err(EngineError::unreadable(
                "input authority belongs to another account",
            ));
        }
        Ok(())
    }

    /// Live restoration of every original boundary after all saved-bundle lookups/waits.
    /// Existing origin labels and conjunctive caller set remain unchanged and unadmitted.
    pub(super) async fn restore_current(
        &self,
        conn: &mut Conn,
        user: Id,
        clock: &dyn Clock,
    ) -> Result<Option<RestoredInputAuthority>, EngineError> {
        let Some(mut restored) = self.restore(conn, user, clock.time()).await? else {
            return Ok(None);
        };
        for origin in &mut restored.origins {
            let Some(current) = CapturedCaller::capture(&origin.caller)
                .restore_current(conn, clock)
                .await?
            else {
                return Ok(None);
            };
            origin.caller = current;
        }
        if restored
            .origins
            .iter()
            .any(|origin| !origin.caller.key_deadline_allows(clock))
        {
            return Ok(None);
        }
        Ok(Some(restored))
    }

    /// Recheck current key validity/policy through the shared credential restoration helper.
    /// No operation admission or target ancestry survives this checkpoint.
    pub(super) async fn restore(
        &self,
        conn: &mut Conn,
        user: Id,
        now: Time,
    ) -> Result<Option<RestoredInputAuthority>, EngineError> {
        self.validate_actor(user)?;
        let mut origins =
            Vec::with_capacity(self.credentials.len() + self.boundaries.len() + self.origins.len());
        for credential in &self.credentials {
            let accepted = match credential {
                CredentialAuthority::Session { as_admin } => {
                    Caller::session(user).with_admin_mode(*as_admin)
                }
                CredentialAuthority::Token {
                    token_id,
                    scopes,
                    node_id,
                } => {
                    let mut grant = TokenGrant::from_stored(scopes, *node_id)
                        .map_err(EngineError::unreadable)?;
                    if let Some(id) = token_id {
                        grant = grant.for_token(*id);
                    }
                    Caller::token(user, grant)
                }
            };
            let Some(caller) = CapturedCaller::capture(&accepted)
                .restore(conn, now)
                .await?
            else {
                return Ok(None);
            };
            // A migrated key cannot supply an operation name missing from accepted old work.
            if caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
            {
                return Ok(None);
            }
            origins.push(RestoredInputOrigin {
                operation: None,
                caller,
            });
        }
        for boundary in &self.boundaries {
            let Some(caller) = boundary.restore(conn, now).await? else {
                return Ok(None);
            };
            // No exact operation was accepted for this boundary; a current rule policy
            // cannot be admitted by guessing one from the later work.
            if caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
            {
                return Ok(None);
            }
            origins.push(RestoredInputOrigin {
                operation: None,
                caller,
            });
        }
        for origin in &self.origins {
            let Some(caller) = origin.restore(conn, now).await? else {
                return Ok(None);
            };
            origins.push(RestoredInputOrigin {
                operation: Some(origin.operation().to_owned()),
                caller,
            });
        }
        Ok(Some(RestoredInputAuthority {
            actor: user,
            captured: self.clone(),
            origins,
        }))
    }
}

/// Record the trusted accepted operation, or the full boundary without inventing a selection.
pub(super) fn capture(caller: &Caller) -> Result<Json, EngineError> {
    Ok(serde_json::to_value(InputAuthority::from_caller(caller)?)?)
}

/// Missing provenance remains unknown; an explicitly empty boundary is invalid.
pub(super) fn saved(raw: Option<&Json>) -> Result<Option<InputAuthority>, EngineError> {
    let authority: Option<InputAuthority> = raw.cloned().map(serde_json::from_value).transpose()?;
    if let Some(authority) = &authority {
        authority.validate()?;
    }
    Ok(authority)
}

/// Keep all earlier origins and the editing credential, without intersecting operation names.
pub(super) fn narrowed(raw: Option<&Json>, caller: &Caller) -> Result<Option<Json>, EngineError> {
    let Some(mut authority) = saved(raw)? else {
        return Ok(None);
    };
    authority.validate_actor(caller.user())?;
    let next = InputAuthority::from_caller(caller)?;
    for boundary in next.boundaries {
        if !authority.boundaries.contains(&boundary) {
            authority.boundaries.push(boundary);
        }
    }
    for origin in next.origins {
        if !authority.origins.contains(&origin) {
            authority.origins.push(origin);
        }
    }
    Ok(Some(serde_json::to_value(authority)?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a control fails by panicking")]
mod tests {
    use super::*;
    use orch_access_contract::key_scope::{
        KeyAction, KeyFacet, KeyObjectRef, KeyResolvedTarget, KeyScope, KeyTargetKind,
    };
    use orch_access_contract::permissions::ObjectKind;
    use orch_types::new_id;
    use serde_json::json;

    fn admitted(user: Id, operation: &str) -> Caller {
        Caller::session(user)
            .with_admin_mode(false)
            .admit_facets(&[KeyFacet {
                operation: operation.into(),
                action: KeyAction::Write,
                resource_kind: KeyTargetKind::Object(ObjectKind::Thread),
                target: KeyResolvedTarget {
                    object: KeyObjectRef {
                        kind: KeyTargetKind::Object(ObjectKind::Thread),
                        id: Some(new_id()),
                    },
                    node_ancestors: Vec::new(),
                    document_ancestors: Vec::new(),
                    credential_account: None,
                },
            }])
            .unwrap()
            .into_caller()
    }

    #[test]
    fn legacy_records_remain_readable_but_unknown_and_empty_provenance_do_not_gain_authority() {
        let old = json!({"credentials":[{"type":"session"}]});
        let authority = saved(Some(&old)).unwrap().unwrap();
        assert_eq!(
            authority.credentials,
            vec![CredentialAuthority::Session { as_admin: false }]
        );
        assert!(authority.origins.is_empty());
        assert!(authority.boundaries.is_empty());
        assert!(saved(None).unwrap().is_none());
        for invalid in [
            json!({}),
            json!({"credentials":[],"boundaries":[],"origins":[]}),
            json!({"credentials":[{"type":"session","extra":true}]}),
        ] {
            assert!(saved(Some(&invalid)).is_err());
        }
        assert!(
            narrowed(None, &admitted(new_id(), "ThreadInputUpdate.edit"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn narrowing_keeps_distinct_operations_and_rejects_a_different_account() {
        let user = new_id();
        let create = admitted(user, "ThreadInputCreate");
        let edit = admitted(user, "ThreadInputUpdate.edit");
        let original = capture(&create).unwrap();
        let updated = narrowed(Some(&original), &edit).unwrap().unwrap();
        let decoded = saved(Some(&updated)).unwrap().unwrap();
        assert!(decoded.credentials.is_empty());
        assert!(decoded.boundaries.is_empty());
        assert_eq!(decoded.origins.len(), 2);
        assert_eq!(decoded.origins[0].operation(), "ThreadInputCreate");
        assert_eq!(decoded.origins[1].operation(), "ThreadInputUpdate.edit");
        assert_eq!(
            narrowed(Some(&updated), &edit).unwrap(),
            Some(updated.clone())
        );
        assert!(
            narrowed(
                Some(&updated),
                &admitted(new_id(), "ThreadInputUpdate.edit")
            )
            .is_err()
        );
        assert!(!updated.to_string().contains("node_ancestors"));
    }

    #[test]
    fn an_unadmitted_rule_key_cannot_be_saved_as_a_legacy_credential() {
        let key = Caller::token(
            new_id(),
            TokenGrant::from_stored_scope(&[], None, Some(KeyScope::default()), None).unwrap(),
        );
        assert!(capture(&key).is_err());
        let legacy = Caller::token(new_id(), TokenGrant::everything());
        let saved_legacy = saved(Some(&capture(&legacy).unwrap())).unwrap().unwrap();
        assert!(saved_legacy.credentials.is_empty());
        assert_eq!(
            saved_legacy.boundaries,
            vec![CapturedCaller::capture(&legacy)]
        );
        assert!(saved_legacy.origins.is_empty());
    }

    #[test]
    fn unselected_capture_keeps_every_token_subtree_expiry_and_place_ceiling() {
        let (user, first_id, second_id, first_root, second_root, refused) =
            (new_id(), new_id(), new_id(), new_id(), new_id(), new_id());
        let expiry = Time::new(orch_testkit::clock::start() + chrono::Duration::hours(1));
        let first = Caller::token(
            user,
            TokenGrant::from_stored_scope(
                &["thread.interact".into(), "thread.view".into()],
                Some(first_root),
                None,
                Some(expiry),
            )
            .unwrap()
            .for_token(first_id)
            .kept_out(vec![orch_authz::KeptOut::Refused { org: refused }]),
        );
        let second = Caller::token(
            user,
            TokenGrant::from_stored_scope(
                &["thread.interact".into(), "provider.use".into()],
                Some(second_root),
                None,
                Some(Time::new(expiry.get() + chrono::Duration::hours(1))),
            )
            .unwrap()
            .for_token(second_id),
        );
        let combined = first.intersect_restrictions(&second).unwrap();
        assert!(combined.admitted_operation().is_none());
        let raw = capture(&combined).unwrap();
        assert!(raw.get("credentials").is_none());
        let decoded = saved(Some(&raw)).unwrap().unwrap();
        assert_eq!(decoded.boundaries, vec![CapturedCaller::capture(&combined)]);
        let credential = &raw["boundaries"][0]["credentials"][0];
        let mut tokens: Vec<Id> = serde_json::from_value(credential["token_ids"].clone()).unwrap();
        tokens.sort_unstable();
        let mut expected_tokens = vec![first_id, second_id];
        expected_tokens.sort_unstable();
        assert_eq!(tokens, expected_tokens);
        let mut roots: Vec<Id> =
            serde_json::from_value(credential["other_subtrees"].clone()).unwrap();
        roots.push(serde_json::from_value(credential["node_id"].clone()).unwrap());
        roots.sort_unstable();
        let mut expected_roots = vec![first_root, second_root];
        expected_roots.sort_unstable();
        assert_eq!(roots, expected_roots);
        assert_eq!(credential["scopes"], json!(["thread.interact"]));
        assert_eq!(
            credential["expires_at"],
            serde_json::to_value(expiry).unwrap()
        );
        assert_eq!(
            credential["kept_out"],
            json!([{"kind":"refused","org":refused}])
        );
        assert_eq!(narrowed(Some(&raw), &combined).unwrap(), Some(raw.clone()));
        let next = narrowed(Some(&raw), &second).unwrap().unwrap();
        let next = saved(Some(&next)).unwrap().unwrap();
        assert_eq!(next.boundaries.len(), 2);
        assert_eq!(next.boundaries[0], decoded.boundaries[0]);
        assert!(narrowed(Some(&raw), &Caller::session(new_id())).is_err());
    }
}
