//! What an adapter tells the shared native conformance cases about its harness, in place of the cases' own branches on which harness it is.
//!
//! The cases run against a build of the program and the scripted model, and are the same for every adapter: a
//! session, a turn, an approval, a question. What differs between programs is a handful of facts the cases cannot
//! know: the approval mode a check's session is opened in, how the program names an MCP tool, which paths of the
//! scripted model a quiet start may reach, and what a start leaves behind that proves nothing but the work left the
//! machine. An adapter says each of them in [`crate::adapter::AgentAdapter::native_check`] and
//! [`crate::adapter::AgentAdapter::verify_quiet_start`]; the defaults are what a program that needs no special
//! treatment gets.

use std::collections::BTreeMap;
use std::path::Path;

use orch_types::Json;

use crate::descriptor::ApprovalMode;

/// The request paths of the scripted model that a program's start may reach.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelPaths {
    /// The paths of which at least one must be reached: the program has to get to the model. Empty means the
    /// paths its forwarded protocol uses.
    pub reached: Vec<String>,
    /// Every path a start may reach, by the end of the path; a request elsewhere fails the case. Empty means the
    /// same as `reached`.
    pub allowed: Vec<String>,
    /// Paths a start may also reach that are matched whole, not by their end.
    pub exact: Vec<String>,
}

/// The observed wire facts needed by the fixed-reviewer native case. This is test setup, not
/// an Auto capability declaration: only the program's verified manifest can offer the mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeNotificationReview {
    /// Actual reviewer routing model, observed independently of the generic code-review model.
    pub model: String,
    /// Extra native command arguments that request review of the disposable marker command.
    pub command_arguments: Json,
    /// Structured JSON answer observed to approve a native review.
    pub allow_response: Json,
    /// Structured JSON answer observed to deny a native review.
    pub deny_response: Json,
    /// JSON pointer to the structured output schema on the actual reviewer request.
    pub schema_pointer: String,
    /// Exact native requested output schema from the pinned diagnostic.
    pub response_schema: Json,
    /// JSON-RPC notification identifying the start of this review.
    pub started_method: String,
    /// JSON-RPC notification identifying the completed review.
    pub completed_method: String,
}

/// Evidence forms are native protocol facts, not program names or new approval authorities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeAutoReview {
    /// Existing fixed-reviewer notification case, unchanged.
    Notifications(NativeNotificationReview),
    /// Structurally matched classifier requests and factual denial hooks.
    Classifier(NativeClassifierReview),
}

/// A bounded predicate on the actual controlled-model request. Unknown shapes do not match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeRequestPattern {
    pub equals: BTreeMap<String, Json>,
    pub present: Vec<String>,
    pub absent: Vec<String>,
    pub contains: BTreeMap<String, Vec<String>>,
    pub excludes: BTreeMap<String, Vec<String>>,
}
impl NativeRequestPattern {
    /// Match structure and text together; a model name or queue position is never sufficient.
    #[must_use]
    pub fn matches(&self, body: &Json) -> bool {
        fn text(value: &Json, output: &mut String) {
            match value {
                Json::String(value) => {
                    output.push_str(value);
                    output.push('\n');
                }
                Json::Array(values) => values.iter().for_each(|value| text(value, output)),
                Json::Object(values) => values.values().for_each(|value| text(value, output)),
                _ => {}
            }
        }
        let contents = |path: &str| {
            let mut output = String::new();
            if let Some(value) = body.pointer(path) {
                text(value, &mut output);
            }
            output
        };
        self.equals
            .iter()
            .all(|(path, value)| body.pointer(path) == Some(value))
            && self
                .present
                .iter()
                .all(|path| body.pointer(path).is_some_and(|value| !value.is_null()))
            && self.absent.iter().all(|path| body.pointer(path).is_none())
            && self.contains.iter().all(|(path, needles)| {
                let value = contents(path);
                needles.iter().all(|needle| value.contains(needle))
            })
            && self.excludes.iter().all(|(path, needles)| {
                let value = contents(path);
                needles.iter().all(|needle| !value.contains(needle))
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeClassifierOutcome {
    Allow,
    Deny,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeClassifierStage {
    /// Opaque adapter-owned stage identity; repeated Stage1 fallback remains Stage1.
    pub identity: String,
    pub model: String,
    pub request: NativeRequestPattern,
    pub script: String,
    pub status: u16,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeClassifierCase {
    pub name: String,
    pub session_model: String,
    pub available_models: Vec<String>,
    pub profile_env: BTreeMap<String, String>,
    pub declared_reviewer: String,
    pub outcome: NativeClassifierOutcome,
    pub stages: Vec<NativeClassifierStage>,
    pub initial_request: NativeRequestPattern,
    pub continuation_request: NativeRequestPattern,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeClassifierReview {
    /// Harmless literal command with one __ORCH_MARKER__ placeholder, replaced only by a quoted owned path.
    pub command: String,
    pub marker_contents: String,
    pub command_arguments: Json,
    pub sandbox_mode: crate::descriptor::SandboxMode,
    pub excluded_refusal_fragment: String,
    pub cases: Vec<NativeClassifierCase>,
}
#[derive(Debug, Clone, Copy)]
pub struct NativeReviewRequest<'a> {
    pub path: &'a str,
    pub body: &'a Json,
    pub status: Option<u16>,
    pub queued: bool,
}
#[derive(Debug)]
pub struct NativeClassifierEvidence<'a> {
    pub case: &'a NativeClassifierCase,
    pub command: &'a str,
    pub control: bool,
    pub turn_id: crate::session::TurnId,
    pub native_session_ref: &'a str,
    pub reviewer: Option<&'a crate::review::ApprovalReviewer>,
    pub events: &'a [crate::events::SessionEvent],
    pub requests: &'a [NativeReviewRequest<'a>],
}

/// What an adapter says of its program to the native conformance cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCheck {
    /// The approval mode a check's session is opened in: the one in which the program asks for nothing a case
    /// does not expect.
    pub approval_mode: ApprovalMode,
    /// Variables added to the check's profile, such as a smaller retry count so that a failing case ends soon.
    pub profile_env: BTreeMap<String, String>,
    /// The paths of the scripted model a quiet start may reach.
    pub model_paths: ModelPaths,
    /// Optional observed fixed-reviewer recipe; this never changes Auto admission.
    pub auto_review: Option<NativeAutoReview>,
}

impl Default for NativeCheck {
    fn default() -> Self {
        Self {
            approval_mode: ApprovalMode::Ask,
            profile_env: BTreeMap::new(),
            model_paths: ModelPaths::default(),
            auto_review: None,
        }
    }
}

/// One request the scripted model saw.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedRequest {
    /// Its path.
    pub path: String,
    /// Its parsed body.
    pub body: Json,
}

/// What a start of the program left behind, for [`crate::adapter::AgentAdapter::verify_quiet_start`].
#[derive(Debug, Clone, Copy)]
pub struct QuietEvidence<'a> {
    /// What the scripted model saw.
    pub requests: &'a [ObservedRequest],
    /// The disposable home the program was started with, where it has one.
    pub home: Option<&'a Path>,
    /// The variables each launch of the program had, where the launcher was watched.
    pub launches: &'a [BTreeMap<String, String>],
}
