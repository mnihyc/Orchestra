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
}

impl Default for NativeCheck {
    fn default() -> Self {
        Self {
            approval_mode: ApprovalMode::Ask,
            profile_env: BTreeMap::new(),
            model_paths: ModelPaths::default(),
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
