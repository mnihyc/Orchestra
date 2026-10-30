//! What a request says of the session it belongs to: the thread, the tool set, the directory, the sandbox mode and the mounts.
//!
//! One executor serves every session of its environment, and which tool set a session has travels with the
//! session (in its ticket) and not with the executor. The connector, which relays each exchange of the
//! harness to the executor, therefore says in a header on every request which session the exchange belongs
//! to. The header is [`SESSION_HEADER`]; its value is this structure as JSON, written in base64 with the
//! URL-safe alphabet and no padding, so that a path with any character in it is a legal header value.
//!
//! The header is trusted only because the request carries the executor's token as well, and the token is held
//! by the connector alone: the executor listens on the loopback address and nothing else can speak to it.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_agent_contract::descriptor::SandboxMode;
use orch_driver_contract::mounts::{AttachedMount, MountMode};
use serde::{Deserialize, Serialize};

use crate::tools::ToolSet;

/// The header that carries the session of a request.
pub const SESSION_HEADER: &str = "x-orch-session";

/// The longest value of the header that is read: a thread's directory and a few dozen mounts fit in a
/// fraction of this, and a value near it is a fault of the sender.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

/// The longest thread id that is taken: an id that is a UUID is 36 characters, and the id ends up in a
/// variable and in a file name.
const MAX_THREAD_ID_CHARS: usize = 64;

/// A further place the session may reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    /// The mount's name.
    pub name: String,
    /// Where it is, as an absolute path.
    pub path: String,
    /// Whether nothing may write there.
    pub read_only: bool,
}

impl From<&AttachedMount> for Mount {
    fn from(attached: &AttachedMount) -> Self {
        Self {
            name: attached.name.clone(),
            path: attached.path.clone(),
            read_only: attached.mode == MountMode::ReadOnly,
        }
    }
}

/// The session a request belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionScope {
    /// The thread's id. It is the value of `ORCH_THREAD_ID` in every command of the session, so it is
    /// restricted to letters, digits, `-` and `_`.
    pub thread_id: String,
    /// The tool set the session has.
    pub tool_set: ToolSet,
    /// The thread's directory, as an absolute path.
    pub directory: String,
    /// Where the agent starts inside it, as an absolute path; the directory when it is left out.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The mode the turn runs in.
    pub sandbox_mode: SandboxMode,
    /// The mounts that are attached, further places to reach.
    #[serde(default)]
    pub mounts: Vec<Mount>,
}

/// Why a session's header cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    /// The header is longer than any session's.
    #[error("the session header is longer than {MAX_HEADER_BYTES} bytes")]
    TooLarge,
    /// The value is not base64 of the URL-safe alphabet.
    #[error("the session header is not base64")]
    NotBase64,
    /// The text is not the session's JSON.
    #[error("the session header is not a session: {0}")]
    NotASession(String),
    /// The thread id is empty, too long, or has a character a variable or a file name should not.
    #[error("the thread id {0:?} is not 1 to {MAX_THREAD_ID_CHARS} letters, digits, - and _")]
    ThreadId(String),
    /// A path that is to be absolute is not.
    #[error("{field} is not an absolute path: {value:?}")]
    NotAbsolute {
        /// Which member.
        field: &'static str,
        /// What it held.
        value: String,
    },
}

impl SessionScope {
    /// The session as the header carries it.
    #[must_use]
    pub fn encode(&self) -> String {
        // A structure of strings, a list and two names cannot fail to be written as JSON.
        let json = serde_json::to_vec(self).unwrap_or_default();
        URL_SAFE_NO_PAD.encode(json)
    }

    /// The session a header's value carries.
    pub fn decode(value: &str) -> Result<Self, ScopeError> {
        if value.len() > MAX_HEADER_BYTES {
            return Err(ScopeError::TooLarge);
        }
        let json = URL_SAFE_NO_PAD
            .decode(value.trim())
            .map_err(|_| ScopeError::NotBase64)?;
        let scope: Self = serde_json::from_slice(&json)
            .map_err(|error| ScopeError::NotASession(error.to_string()))?;
        scope.check()?;
        Ok(scope)
    }

    /// Where the agent starts: the working directory when the session gives one, else the directory.
    #[must_use]
    pub fn start_directory(&self) -> &str {
        self.cwd.as_deref().unwrap_or(&self.directory)
    }

    /// Checks what the executor relies on: an id that is safe where it ends up, and paths that are
    /// absolute.
    fn check(&self) -> Result<(), ScopeError> {
        let id = &self.thread_id;
        let id_is_safe = !id.is_empty()
            && id.chars().count() <= MAX_THREAD_ID_CHARS
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !id_is_safe {
            return Err(ScopeError::ThreadId(id.clone()));
        }
        let absolute = |field: &'static str, value: &str| {
            if std::path::Path::new(value).is_absolute() && !value.contains('\0') {
                Ok(())
            } else {
                Err(ScopeError::NotAbsolute {
                    field,
                    value: value.to_owned(),
                })
            }
        };
        absolute("directory", &self.directory)?;
        if let Some(cwd) = &self.cwd {
            absolute("cwd", cwd)?;
        }
        for mount in &self.mounts {
            absolute("a mount's path", &mount.path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> SessionScope {
        SessionScope {
            thread_id: "0199abcd-1234-7000-8000-0000000000e1".to_owned(),
            tool_set: ToolSet::ClaudeCode,
            directory: "/workspace/web".to_owned(),
            cwd: Some("/workspace/web/api".to_owned()),
            sandbox_mode: SandboxMode::Workspace,
            mounts: vec![Mount {
                name: "src".to_owned(),
                path: "/data/ünï cödé".to_owned(),
                read_only: true,
            }],
        }
    }

    #[test]
    fn a_session_survives_the_header_it_travels_in_whatever_characters_its_paths_have() {
        let header = scope().encode();
        assert!(header.is_ascii(), "a header value is plain text");
        assert_eq!(SessionScope::decode(&header), Ok(scope()));
    }

    #[test]
    fn the_session_is_written_as_the_connector_is_told_to_write_it() {
        let json = serde_json::json!({
            "thread_id": "t-1", "tool_set": "plain", "directory": "/w", "sandbox_mode": "open"
        });
        let header = URL_SAFE_NO_PAD.encode(json.to_string());
        let read = SessionScope::decode(&header).expect("the smallest session");
        assert_eq!(read.tool_set, ToolSet::Plain);
        assert_eq!(read.start_directory(), "/w");
        assert!(read.mounts.is_empty());
    }

    #[test]
    fn a_header_that_is_not_a_session_is_refused_with_the_reason() {
        assert_eq!(
            SessionScope::decode("not base64 !!"),
            Err(ScopeError::NotBase64)
        );
        let unknown = serde_json::json!({
            "thread_id": "t", "tool_set": "plain", "directory": "/w", "sandbox_mode": "open", "network": false
        });
        let header = URL_SAFE_NO_PAD.encode(unknown.to_string());
        assert!(
            matches!(
                SessionScope::decode(&header),
                Err(ScopeError::NotASession(_))
            ),
            "a member this executor does not know may be a restriction it cannot hold"
        );
        assert_eq!(
            SessionScope::decode(&"A".repeat(MAX_HEADER_BYTES + 1)),
            Err(ScopeError::TooLarge)
        );
    }

    #[test]
    fn a_thread_id_that_could_not_be_trusted_in_a_variable_or_a_file_name_is_refused() {
        let too_long = "t".repeat(65);
        for bad in ["", "a/b", "..", "a b", "x\n", too_long.as_str()] {
            let mut session = scope();
            session.thread_id = bad.to_owned();
            assert!(
                matches!(
                    SessionScope::decode(&session.encode()),
                    Err(ScopeError::ThreadId(_))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_relative_path_for_the_directory_the_working_directory_or_a_mount_is_refused() {
        let mut session = scope();
        session.directory = "web".to_owned();
        assert!(matches!(
            SessionScope::decode(&session.encode()),
            Err(ScopeError::NotAbsolute {
                field: "directory",
                ..
            })
        ));
        let mut session = scope();
        session.cwd = Some("api".to_owned());
        assert!(matches!(
            SessionScope::decode(&session.encode()),
            Err(ScopeError::NotAbsolute { field: "cwd", .. })
        ));
        let mut session = scope();
        session.mounts[0].path = "relative".to_owned();
        assert!(matches!(
            SessionScope::decode(&session.encode()),
            Err(ScopeError::NotAbsolute { .. })
        ));
    }

    #[test]
    fn an_attached_mount_that_is_read_only_is_read_only_here_and_the_others_may_be_written() {
        let attached = |mode| AttachedMount {
            name: "m".to_owned(),
            path: "/m".to_owned(),
            mode,
        };
        assert!(Mount::from(&attached(MountMode::ReadOnly)).read_only);
        assert!(!Mount::from(&attached(MountMode::Shared)).read_only);
        assert!(!Mount::from(&attached(MountMode::Private)).read_only);
    }
}
