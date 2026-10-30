//! The names, limits and profile options of this adapter, each with the reason for its value.
//!
//! Everything that shapes behaviour and is not read from a request is here, so that a number never hides in
//! the code that uses it. The profile of an agent may override the few options of [`ProfileConfig`]; the
//! rest is fixed by what the harness does and by what Orch promises around it.

use std::time::Duration;

use orch_types::Json;
use serde::Deserialize;

/// The harness kind this adapter drives.
pub const KIND: &str = "claude-code";

/// The version of this adapter's declarations (`AgentDescriptor.adapter_version`).
pub const ADAPTER_VERSION: u32 = 1;

/// What a subscription of the harness is called where one is added or listed.
pub const SUBSCRIPTION_TITLE: &str = "Claude subscription";

/// The exact candidate harness pin. A wider range needs real contract/startup evidence for every
/// advertised version, including the native instruction and quiet-start configuration.
pub const SUPPORTED_RANGE: &str = "=2.1.312";

/// The candidate whose same-version native transcript is being checked before support is advertised.
pub(crate) const PORTABLE_ARCHIVE_CANDIDATE: &str = "2.1.312";

/// The program looked up on the search path when no binary is given.
pub const PROGRAM: &str = "claude";

/// The format of an exported transcript. Named for the harness and for the line of versions it was written
/// by: a transcript from another line is refused on import.
pub const TRANSCRIPT_FORMAT: &str = "claude-code-jsonl-v1";

/// The directory name under the harness's `projects` directory in which every session of a home managed by
/// Orch keeps its transcript, whatever its working directory (`CLAUDE_CODE_PROJECT_DIR_NAME`). One place
/// means a session is found by its id alone.
pub const MANAGED_PROJECT_DIR: &str = "orch";

/// The directory of a home managed by Orch under which the harness keeps what the libraries it is made of keep
/// by the freedesktop rules where nothing says otherwise: a cache, its logs, data and configuration of their own.
/// Left alone they go below the account's home (`~/.cache`), among what people keep there, and are not in the home
/// the harness was given.
pub const MANAGED_XDG_DIR: &str = "xdg";

/// The variables of those rules, each with the directory below [`MANAGED_XDG_DIR`] it is pointed at.
pub const MANAGED_XDG_PLACES: [(&str, &str); 4] = [
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
];

/// How long an idle process of a session that works through Orch's executor stays alive: long enough that
/// a quick follow-up finds it, short enough that an idle thread holds no memory (the harness is about a
/// gibibyte for each running agent).
pub const DEFAULT_IDLE_EXIT_SECS: u64 = 300;

/// How long a turn may go without a word from the harness, while the harness waits for the model and for nothing
/// else, before the turn is ended and the process with it: fifteen minutes. The harness has deadlines of its own
/// for a model that does not answer (ten minutes for a request), so this is the one for a harness that has none
/// left; a model that thinks for a long time still says that it is alive, and a command that runs for a long time
/// is not waited for here at all.
pub const DEFAULT_SILENCE_SECS: u64 = 15 * 60;

/// How long the harness may go on announcing that it tries again, with nothing else between, before the turn is
/// ended and the process with it: ten minutes. Its own limit is ten tries over about two and a half minutes, so
/// this is only for a harness that is set to try longer, or for one that does not stop.
pub const DEFAULT_RETRY_PATIENCE_SECS: u64 = 10 * 60;

/// How long a process is given to end after its input is closed before it is killed: it has nothing left
/// to do, so a few seconds are plenty, and a hung one must not hold a session open.
pub const EXIT_GRACE: Duration = Duration::from_secs(8);

/// How long a process is given to answer the handshake that follows its start: the harness is a large
/// program that loads its settings, plugins and servers first.
pub const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the question of the harness's version, or of a sign-in, may take.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an added message may wait to be taken into the running turn before the call that added it
/// answers that it is queued. The link gives such a call thirty seconds; this leaves it room to answer.
pub const STEER_ACK_WAIT: Duration = Duration::from_secs(20);

/// How long a count of what fills the context is waited for: the harness asks the vendor's token counter
/// about each part of it, a second or two each. The link gives the command thirty seconds; this leaves it
/// room to answer that there is none.
pub const CONTEXT_COUNT_WAIT: Duration = Duration::from_secs(20);

/// How many lines of the harness's standard error are kept to explain a death.
pub const STDERR_KEPT_LINES: usize = 40;

/// The longest line the harness may write: a tool result can carry a whole file or an image, and nothing
/// the harness writes is larger than a request it can make of its model (32 MiB) plus its framing.
pub const FRAME_MAX_BYTES: usize = 64 * 1024 * 1024;

/// How many failed attempts in a row the harness announces before a turn says so to the people watching:
/// the first retries are routine, the fifth is not.
pub const RETRY_NOTICE_ATTEMPT: u64 = 5;

/// What the harness's `fast_mode_state` says when the vendor's limit for the fast mode was reached and the harness went on at
/// the ordinary speed by itself (the other states are `on` and `off`).
pub const FAST_MODE_PAUSED: &str = "cooldown";

/// The largest frame that is kept whole as a diagnostic; a larger one is kept as a stub that says what it
/// was and how large, so that a file read by the agent does not fill the diagnostics.
pub const RAW_FRAME_MAX_BYTES: usize = 256 * 1024;

/// The profile's own options (`AgentProfile.config`), merged over the defaults. A value that is not of the
/// right type is ignored, so that a mistyped option never stops a harness from starting.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProfileConfig {
    /// Seconds an idle process stays alive when the harness works through Orch's executor; `0` keeps it.
    pub idle_exit_secs: u64,
    /// Whether a personal harness may run agents and commands in the background. Managed sessions always
    /// disable this: a native wake-up has no Orch input or model lease.
    pub background_tasks: bool,
    /// Whether the harness's own to-do tools are offered, which is what a plan is made of.
    pub todo_tools: bool,
    /// Which settings files the harness reads, as it names them (`user`, `project`, `local`, separated by
    /// commas). Only the project's own by default: in a runtime that is the instruction file, the skills
    /// and the settings of the thread's directory and nothing of anyone's home.
    pub setting_sources: String,
    /// Seconds a turn may go without a word from the harness, while it waits for the model, before the turn is
    /// ended and the process stopped; `0` never.
    pub silence_secs: u64,
    /// Seconds the harness may go on trying again, with nothing else between, before the turn is ended and the
    /// process stopped; `0` never.
    pub retry_patience_secs: u64,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            idle_exit_secs: DEFAULT_IDLE_EXIT_SECS,
            background_tasks: false,
            todo_tools: true,
            setting_sources: "project".to_owned(),
            silence_secs: DEFAULT_SILENCE_SECS,
            retry_patience_secs: DEFAULT_RETRY_PATIENCE_SECS,
        }
    }
}

impl ProfileConfig {
    /// The options a profile's `config` states; absent or unreadable ones keep their defaults.
    #[must_use]
    pub fn from_json(config: &Json) -> Self {
        let mut options = Self::default();
        let Some(map) = config.as_object() else {
            return options;
        };
        if let Some(secs) = map.get("idle_exit_secs").and_then(Json::as_u64) {
            options.idle_exit_secs = secs;
        }
        if let Some(on) = map.get("background_tasks").and_then(Json::as_bool) {
            options.background_tasks = on;
        }
        if let Some(on) = map.get("todo_tools").and_then(Json::as_bool) {
            options.todo_tools = on;
        }
        if let Some(secs) = map.get("silence_secs").and_then(Json::as_u64) {
            options.silence_secs = secs;
        }
        if let Some(secs) = map.get("retry_patience_secs").and_then(Json::as_u64) {
            options.retry_patience_secs = secs;
        }
        if let Some(sources) = map.get("setting_sources").and_then(Json::as_str) {
            let known = ["user", "project", "local"];
            if !sources.is_empty()
                && sources
                    .split(',')
                    .all(|source| known.contains(&source.trim()))
            {
                options.setting_sources = sources.replace(' ', "");
            }
        }
        options
    }

    /// How long a turn may be silent, `None` for as long as it takes.
    #[must_use]
    pub fn silence(&self) -> Option<Duration> {
        (self.silence_secs > 0).then(|| Duration::from_secs(self.silence_secs))
    }

    /// How long the harness may try again without end, `None` for as long as it takes.
    #[must_use]
    pub fn retry_patience(&self) -> Option<Duration> {
        (self.retry_patience_secs > 0).then(|| Duration::from_secs(self.retry_patience_secs))
    }

    /// How long an idle process is kept, `None` for always.
    #[must_use]
    pub fn idle_exit(&self) -> Option<Duration> {
        (self.idle_exit_secs > 0).then(|| Duration::from_secs(self.idle_exit_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_profile_that_says_nothing_gets_the_defaults() {
        let options = ProfileConfig::from_json(&json!({}));
        assert_eq!(options, ProfileConfig::default());
        assert_eq!(options.idle_exit(), Some(Duration::from_secs(300)));
        assert!(!options.background_tasks);
        assert_eq!(options.silence(), Some(Duration::from_secs(900)));
        assert_eq!(options.retry_patience(), Some(Duration::from_secs(600)));
    }

    #[test]
    fn what_a_profile_states_replaces_the_default_and_a_wrong_type_is_ignored() {
        let options = ProfileConfig::from_json(&json!({
            "idle_exit_secs": 0, "background_tasks": true, "todo_tools": "yes"
        }));
        assert_eq!(options.idle_exit(), None, "zero keeps the process");
        assert!(options.background_tasks);
        assert!(
            options.todo_tools,
            "a text is no boolean, so the default stays"
        );
        assert_eq!(
            ProfileConfig::from_json(&json!("not an object")),
            ProfileConfig::default()
        );
        let patient =
            ProfileConfig::from_json(&json!({ "silence_secs": 0, "retry_patience_secs": 30 }));
        assert_eq!(
            (patient.silence(), patient.retry_patience()),
            (None, Some(Duration::from_secs(30)))
        );
    }
}
