//! Orch's own messages on top of the protocol: what a companion plugin of an agent announces and sends.
//!
//! Agent Client Protocol reserves method names that start with an underscore, and a `_meta` member on every
//! message, for what a program adds. A program that runs a companion plugin of Orch's (a piece of code Orch ships
//! to run inside the program) uses them to give what the protocol lacks, and to say, in one `_orch/hello`, which of
//! those extras work in this process. Each extra turns on the claims it exists for; one that does not work is off
//! alone, with its reason, while the rest of the protocol goes on.
//!
//! The names and the shapes are the plugin's (`extensions/agents/acp/dsh-bridge/src/protocol.ts` for DeepSeek
//! Harness); this module reads what the adapter needs of them.
//!
//! | Name | Direction | Meaning |
//! |------|-----------|---------|
//! | `_orch/hello` | agent notifies, or client requests | `{ protocol, plugin, host, extras: { name: { status, reason?, detail? } }, methods, emits }` |
//! | `_orch/extra_state` | agent notifies | `{ extra, state }`: an extra went off while running |
//! | `_orch/stream` | agent notifies | `{ sessionId, turn, step, attempt, seq, frames: [ { kind: start, text, reasoning, tool_input, end } ] }` |
//! | `_orch/input/steer`, `_orch/input/followup` | client requests | `{ sessionId, messageId, content: [ parts ] }`, answered `{ messageId, accepted }` |
//! | `_orch/input/state` | agent notifies | what became of a message |
//! | `_orch/question/ask` | agent requests | `{ sessionId, callId, questions }`, answered `{ answers }`; `_orch/question/closed` when it is not waited for |
//! | `_orch/plan/set`, `_orch/plan/get` | client requests | plan mode on or off |
//! | `_orch/plan/review` | agent requests | `{ sessionId, callId, plan }`, answered `{ decision, feedback? }` |
//! | `_orch/plan/state`, `_orch/plan/refused` | agent notifies | plan mode changed; a call was refused because of it |
//! | `_orch/approval/set` | client requests | `{ sessionId, mode: ask or bypass }` |
//! | `_orch/usage` | agent notifies | one model call with its token counts |
//! | `_orch/compaction` | agent notifies | the phases of a compaction |
//! | `_orch/history/import`, `_orch/session/fork` | client requests | a neutral history into a new session; a copy of a session |
//!
//! Beyond these, the adapter sends the plugin what it needs in `_meta.orch` of `session/new`, `session/resume` and
//! `session/prompt`: the thread and segment, the turn, the approval and sandbox modes, the directory of the
//! session's skills, and, where the arrangement has the program's tools reach the environment through Orch's
//! executor, the address and the token of that executor. A person's words with a refusal (Keep planning) travel in
//! `_meta.orch.note` of a permission response's outcome.

use std::collections::BTreeMap;

use orch_agent_contract::history::{Dropped, Fidelity, HistoryReport, KindCounts};
use orch_agent_contract::requests::UsageRequest;
use orch_types::Json;
use serde::{Deserialize, Serialize};

/// The version of what the adapter puts in `_meta.orch`.
pub const VERSION: u32 = 1;

/// The version of the plugin's protocol this adapter reads.
pub const PROTOCOL: u32 = 1;

/// The format of the neutral history file the plugin reads.
pub const NEUTRAL_FORMAT: &str = "orch-neutral-jsonl-v1";

/// What a companion plugin can give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Extra {
    /// Text and reasoning arrive as they are written.
    Streaming,
    /// A message can be added to a running turn.
    Steering,
    /// The agent's questions to a person arrive as questions.
    Questions,
    /// The agent can plan, with nothing changed until a person proceeds.
    PlanMode,
    /// Changes ask for approval, or run, as Orch says for the session.
    Approvals,
    /// Every model call is reported with its tokens.
    Usage,
    /// The agent says when it compacts.
    Compaction,
    /// A session's skills are read from the directory Orch lays out.
    Skills,
    /// A session can be copied.
    Fork,
    /// Orch's neutral history can be written into a session.
    NativeCarry,
    /// The agent's shell and files can run in the environment through Orch's executor.
    ExecutorBackends,
}

impl Extra {
    /// Every extra, in the order the plugin reports them.
    pub const ALL: [Self; 11] = [
        Self::Streaming,
        Self::Steering,
        Self::Questions,
        Self::PlanMode,
        Self::Approvals,
        Self::Usage,
        Self::Compaction,
        Self::Skills,
        Self::Fork,
        Self::NativeCarry,
        Self::ExecutorBackends,
    ];

    /// The name as it is written.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Streaming => "streaming",
            Self::Steering => "steering",
            Self::Questions => "questions",
            Self::PlanMode => "plan_mode",
            Self::Approvals => "approvals",
            Self::Usage => "usage",
            Self::Compaction => "compaction",
            Self::Skills => "skills",
            Self::Fork => "fork",
            Self::NativeCarry => "native_carry",
            Self::ExecutorBackends => "executor_backends",
        }
    }

    /// The extra of a name, if it is one.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|extra| extra.name() == name)
    }
}

/// Whether one extra works in the plugin's process.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ExtraState {
    /// `on` or `off`.
    pub status: String,
    /// Why it is off.
    pub reason: Option<String>,
    /// What the plugin found.
    pub detail: Option<String>,
}

impl ExtraState {
    /// Whether it works.
    #[must_use]
    pub fn on(&self) -> bool {
        self.status == "on"
    }
}

/// A plugin's name and version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Plugin {
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
}

/// The program the plugin runs in, as the plugin read it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Host {
    /// Its version, where the plugin could read it.
    pub version: Option<String>,
    /// Whether the plugin's own list of builds it was written against holds this one.
    pub tested: bool,
}

/// The plugin's hello.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Hello {
    /// The version of the protocol the plugin speaks.
    pub protocol: u32,
    /// Who it is.
    pub plugin: Plugin,
    /// The program it runs in.
    pub host: Host,
    /// The state of every extra, by name.
    pub extras: BTreeMap<String, ExtraState>,
    /// The requests of the client it answers.
    pub methods: Vec<String>,
}

/// What the manifest and the plugin together switched on and off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Accepted {
    /// The extras that are on and that the manifest accepts.
    pub on: Vec<Extra>,
    /// The extras that are not, each with the reason in a few words.
    pub off: Vec<(String, String)>,
}

impl Hello {
    /// Reads a hello from the parameters of `_orch/hello`, from its answer, or from `_meta.orch` of an answer.
    #[must_use]
    pub fn read(value: &Json) -> Option<Self> {
        let hello: Self = serde_json::from_value(value.clone()).ok()?;
        (hello.protocol != 0).then_some(hello)
    }

    /// The extras that work and that the manifest accepts, and the others with the reason each is not on.
    #[must_use]
    pub fn accepted(&self, allowed: &[String]) -> Accepted {
        let mut found = Accepted::default();
        if self.protocol != PROTOCOL {
            for name in self.extras.keys() {
                found.off.push((
                    name.clone(),
                    format!(
                        "the plugin speaks version {} of its protocol, and this adapter reads version {PROTOCOL}",
                        self.protocol
                    ),
                ));
            }
            return found;
        }
        for (name, state) in &self.extras {
            let known = Extra::parse(name);
            if !allowed.iter().any(|allowed| allowed == name) {
                found.off.push((
                    name.clone(),
                    "this program's manifest does not accept it".to_owned(),
                ));
            } else if let Some(extra) = known {
                if state.on() {
                    found.on.push(extra);
                } else {
                    found.off.push((
                        name.clone(),
                        state
                            .reason
                            .clone()
                            .unwrap_or_else(|| "the plugin reports it off".to_owned()),
                    ));
                }
            } else {
                found
                    .off
                    .push((name.clone(), "this adapter does not know it".to_owned()));
            }
        }
        found.on.sort();
        found
    }
}

/// The `_meta.orch` object the adapter sends with a session or a prompt.
#[must_use]
pub fn meta(members: Json) -> Json {
    serde_json::json!({ "orch": members })
}

// -- streaming --------------------------------------------------------------------------------------------

/// One frame of a live model call.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    /// The call began.
    Start,
    /// Text of the answer.
    Text {
        /// What was added.
        delta: String,
    },
    /// Text of the reasoning.
    Reasoning {
        /// What was added.
        delta: String,
    },
    /// The arguments of a tool call as they are written.
    ToolInput,
    /// The call ended.
    End {
        /// `committed` or `abandoned`.
        outcome: String,
    },
    /// A kind this adapter does not know.
    #[serde(other)]
    Other,
}

/// The parameters of `_orch/stream`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StreamNotice {
    /// The model call the frames belong to.
    pub attempt: String,
    /// The position of the first frame among the frames of the call, dense from zero.
    pub seq: u64,
    /// The frames.
    pub frames: Vec<Frame>,
}

impl StreamNotice {
    /// Reads the parameters; none where there is no call named.
    #[must_use]
    pub fn read(params: &Json) -> Option<Self> {
        let notice: Self = serde_json::from_value(params.clone()).ok()?;
        (!notice.attempt.is_empty()).then_some(notice)
    }
}

/// What a committed text or thought update says of the call it ends: the plugin's `_meta.orch.attempt`.
#[must_use]
pub fn committed_attempt(update: &Json) -> Option<&str> {
    update
        .pointer("/_meta/orch/attempt")
        .and_then(Json::as_str)
        .filter(|attempt| !attempt.is_empty())
}

// -- usage ------------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Counts {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    total_tokens: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UsageNotice {
    turn: i64,
    step: i64,
    attempt: Option<String>,
    model: Option<String>,
    usage: Option<Counts>,
    interrupted: bool,
    failed: bool,
}

/// One model call of the plugin, as the request the ledger keeps.
///
/// The plugin's counts are disjoint (the input is what was not read from or written to a cache); the ledger's
/// input includes both cache counts, so they are added back. A count the provider did not report stays absent.
#[must_use]
pub fn usage_request(params: &Json) -> Option<UsageRequest> {
    let notice: UsageNotice = serde_json::from_value(params.clone()).ok()?;
    let id = notice
        .attempt
        .clone()
        .filter(|attempt| !attempt.is_empty())
        .unwrap_or_else(|| format!("call:{}:{}", notice.turn, notice.step));
    let mut request = UsageRequest {
        id,
        model: notice.model.filter(|model| !model.is_empty()),
        ..UsageRequest::default()
    };
    if let Some(counts) = notice.usage {
        let read = counts.cache_read_tokens;
        let write = counts.cache_write_tokens;
        request.tokens.input_tokens = counts
            .input_tokens
            .map(|fresh| fresh + read.unwrap_or(0) + write.unwrap_or(0));
        request.tokens.cached_input_tokens = read;
        request.tokens.cache_write_tokens = write;
        request.tokens.output_tokens = counts.output_tokens;
        request.tokens.reasoning_tokens = counts.reasoning_tokens.filter(|reasoning| {
            counts
                .output_tokens
                .is_none_or(|output| *reasoning <= output)
        });
        request.tokens.total_tokens = counts.total_tokens.or_else(|| {
            request
                .tokens
                .input_tokens
                .zip(request.tokens.output_tokens)
                .map(|(input, output)| input + output)
        });
    }
    if notice.interrupted {
        request.finish(
            "forced",
            Some("The turn was stopped while the model wrote."),
            None,
        );
    } else if notice.failed {
        request.finish(
            "harness_error",
            Some("The model request did not complete."),
            None,
        );
    } else {
        request.finish("done", None, None);
    }
    Some(request)
}

// -- history ----------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ReportKind {
    kept: i64,
    converted: i64,
    dropped: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ReportDropped {
    kind: String,
    count: i64,
    reason: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Report {
    fidelity: String,
    items_total: i64,
    by_kind: BTreeMap<String, ReportKind>,
    dropped: Vec<ReportDropped>,
    warnings: Vec<String>,
}

/// What the plugin says an import did, as Orch's report of it.
#[must_use]
pub fn history_report(value: &Json) -> Option<HistoryReport> {
    let report: Report = serde_json::from_value(value.clone()).ok()?;
    let fidelity = match report.fidelity.as_str() {
        "lossless" => Fidelity::Lossless,
        "structured" => Fidelity::Structured,
        "summary" => Fidelity::Summary,
        _ => return None,
    };
    Some(HistoryReport {
        fidelity,
        items_total: report.items_total,
        by_kind: report
            .by_kind
            .into_iter()
            .map(|(kind, counts)| {
                (
                    kind,
                    KindCounts {
                        kept: counts.kept,
                        converted: counts.converted,
                        dropped: counts.dropped,
                    },
                )
            })
            .collect(),
        dropped: report
            .dropped
            .into_iter()
            .map(|dropped| Dropped {
                kind: dropped.kind,
                count: dropped.count,
                reason: dropped.reason,
            })
            .collect(),
        warnings: report.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hello() -> Hello {
        Hello::read(&json!({
            "protocol": 1,
            "plugin": { "name": "orch-dsh-bridge", "version": "0.1.0" },
            "host": { "name": "dsh", "version": "0.2.1-rc.2", "tested": true },
            "extras": {
                "steering": { "status": "on" },
                "questions": { "status": "off", "reason": "host_api_missing", "detail": "no ask tool" },
                "usage": { "status": "on" },
                "teleport": { "status": "on" },
                "fork": { "status": "on" }
            },
            "methods": ["_orch/input/steer"]
        }))
        .unwrap()
    }

    #[test]
    fn a_hello_gives_the_extras_that_are_on_and_the_manifest_accepts_and_the_reason_for_the_rest() {
        let allowed: Vec<String> = ["steering", "questions", "usage"]
            .map(str::to_owned)
            .to_vec();
        let found = hello().accepted(&allowed);
        assert_eq!(found.on, [Extra::Steering, Extra::Usage]);
        let off: BTreeMap<_, _> = found.off.into_iter().collect();
        assert_eq!(off["questions"], "host_api_missing");
        assert!(
            off["teleport"].contains("manifest"),
            "an extra no manifest accepts says so"
        );
        assert!(off["fork"].contains("manifest"));
    }

    #[test]
    fn a_hello_of_another_protocol_version_gives_nothing_and_a_hello_without_one_is_none() {
        let mut other = hello();
        other.protocol = 2;
        let found = other.accepted(&["steering".to_owned()]);
        assert!(found.on.is_empty());
        assert!(found.off.iter().all(|(_, why)| why.contains("version 2")));
        assert!(Hello::read(&json!({ "extras": {} })).is_none());
    }

    #[test]
    fn every_extra_is_known_by_its_written_name() {
        for extra in Extra::ALL {
            assert_eq!(Extra::parse(extra.name()), Some(extra));
        }
        assert_eq!(Extra::parse("nothing"), None);
    }

    #[test]
    fn a_stream_notice_reads_its_frames_and_skips_a_kind_it_does_not_know() {
        let notice = StreamNotice::read(&json!({
            "sessionId": "s", "turn": 1, "step": 0, "attempt": "a1", "seq": 4,
            "frames": [
                { "kind": "text", "index": 0, "delta": "Hel" },
                { "kind": "reasoning", "index": 0, "delta": "hm" },
                { "kind": "tool_input", "index": 1, "callId": "c", "delta": "{" },
                { "kind": "sparkle" },
                { "kind": "end", "outcome": "committed" }
            ]
        }))
        .unwrap();
        assert_eq!((notice.attempt.as_str(), notice.seq), ("a1", 4));
        assert_eq!(notice.frames.len(), 5);
        assert_eq!(
            notice.frames[0],
            Frame::Text {
                delta: "Hel".into()
            }
        );
        assert_eq!(notice.frames[3], Frame::Other);
        assert!(StreamNotice::read(&json!({ "frames": [] })).is_none());
        assert_eq!(
            committed_attempt(
                &json!({ "_meta": { "orch": { "attempt": "a1", "turn": 1, "step": 0 } } })
            ),
            Some("a1")
        );
    }

    #[test]
    fn disjoint_counts_become_the_ledgers_with_the_cache_inside_the_input_and_nothing_invented() {
        let request = usage_request(&json!({
            "sessionId": "s", "turn": 2, "step": 1, "attempt": "a9", "provider": "orch", "model": "m",
            "usage": { "inputTokens": 100, "outputTokens": 30, "cacheReadTokens": 400, "cacheWriteTokens": 20, "reasoningTokens": 10 }
        }))
        .unwrap();
        assert_eq!(request.id, "a9");
        assert_eq!(request.model.as_deref(), Some("m"));
        assert_eq!(request.tokens.input_tokens, Some(520));
        assert_eq!(request.tokens.cached_input_tokens, Some(400));
        assert_eq!(request.tokens.cache_write_tokens, Some(20));
        assert_eq!(request.tokens.output_tokens, Some(30));
        assert_eq!(request.tokens.reasoning_tokens, Some(10));
        assert_eq!(request.tokens.total_tokens, Some(550));
        assert_eq!(request.check(), Ok(()));
        let none = usage_request(
            &json!({ "sessionId": "s", "turn": 1, "step": 3, "provider": "p", "model": "m" }),
        )
        .unwrap();
        assert_eq!(none.id, "call:1:3");
        assert_eq!(
            none.tokens.input_tokens, None,
            "a call with no report has no counts"
        );
        assert_eq!(none.check(), Ok(()));
    }

    #[test]
    fn an_interrupted_call_and_a_failed_one_end_with_their_own_outcome() {
        let stopped = usage_request(&json!({ "turn": 1, "step": 0, "interrupted": true })).unwrap();
        assert_eq!(
            stopped
                .outcome
                .as_ref()
                .map(|outcome| outcome.code().to_owned())
                .as_deref(),
            Some("forced")
        );
        let failed = usage_request(&json!({ "turn": 1, "step": 0, "failed": true })).unwrap();
        assert_eq!(
            failed
                .outcome
                .as_ref()
                .map(|outcome| outcome.code().to_owned())
                .as_deref(),
            Some("harness_error")
        );
    }

    #[test]
    fn a_history_report_of_the_plugin_is_read_as_orchs() {
        let report = history_report(&json!({
            "fidelity": "structured", "itemsTotal": 7,
            "byKind": { "agent_message": { "kept": 3, "converted": 0, "dropped": 0 }, "plan": { "kept": 0, "converted": 0, "dropped": 1 } },
            "dropped": [ { "kind": "plan", "count": 1, "reason": "dsh has no plan item" } ],
            "warnings": ["w"]
        }))
        .unwrap();
        assert_eq!(report.fidelity, Fidelity::Structured);
        assert_eq!(report.items_total, 7);
        assert_eq!(report.by_kind["agent_message"].kept, 3);
        assert_eq!(report.dropped[0].count, 1);
        assert!(history_report(&json!({ "fidelity": "perfect" })).is_none());
    }
}
