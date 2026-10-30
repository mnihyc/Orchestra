//! Native and neutral archives, import fidelity and reports.
//!
//! A conversation can leave one harness process and arrive in another: when a thread is switched to another
//! provider or profile, forked, or moved with its environment. How well it survives depends on what the
//! two sides can do, and the answer is declared by their descriptors (`export_native`, `import_native`,
//! `import_neutral`, `fork_native`). This module holds the two archives a session can be exported as
//! ([`ExportKind`], [`SessionArchive`]), how faithfully a history arrived ([`Fidelity`]) and the report that
//! says what was kept, converted and dropped ([`HistoryReport`], built with [`ReportBuilder`]), and the rule
//! that chooses the method for a request ([`choose_method`]).
//!
//! A **native** archive is the harness's own record of the session, in a format named for the harness and
//! its version (`claude-code-jsonl-v1`); it is opaque to Orch and only a harness of the same kind and a version
//! it reads can take it. A harness that keeps a session in a store of its own, which cannot be given back whole
//! (as Codex does), declares no native export: it forks and resumes a session where its store is, and into
//! another home a conversation goes as the neutral archive. The **neutral** archive is the same conversation
//! as JSON Lines of items, the main stream only, oldest first: every adapter that declares `import_neutral`
//! reads it.

use std::collections::BTreeMap;

use orch_types::{Hash, Id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::descriptor::Capabilities;

/// The format of the neutral archive: items, one JSON object per line, the main stream, oldest first.
pub const NEUTRAL_FORMAT: &str = "orch-neutral-jsonl-v1";

/// The exact retained native lane whose user-input boundary is inspected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativePrefixSource {
    /// Its Orch thread; naming this lane grants no authority over it.
    pub thread_id: Id,
    /// Its immutable source segment.
    pub segment_id: Id,
    /// The session identifier actually recorded by the harness.
    pub native_session_ref: String,
}

/// Server-verified creation provenance for an exact native history request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeHistorySource {
    /// The exact retained native lane.
    pub source: NativePrefixSource,
    /// The authenticated server verified Orch creation, including every physical alias's adoption provenance.
    pub started_by_orch: bool,
    /// Shares one home metadata snapshot across a background pass's bounded request batches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_pass: Option<Id>,
}

impl From<NativePrefixSource> for NativeHistorySource {
    fn from(source: NativePrefixSource) -> Self {
        Self {
            source,
            started_by_orch: false,
            inventory_pass: None,
        }
    }
}

/// A measured or removed native session, without exposing its home or file paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeHistoryFact {
    /// The exact session requested, even when that item was refused.
    pub source: NativePrefixSource,
    /// Bytes observed before removal, or unknown when measurement was refused.
    pub bytes: Option<i64>,
    /// Physical removal completed or every exact file was already absent.
    pub removed: bool,
    /// A stable refusal or failure code; absent for a successful observation.
    pub error: Option<String>,
}

/// Confirmed old-cache cleanup within one named, documented directory of a managed home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeHomeCleanupItem {
    /// A documented cache directory name, never the physical home path.
    pub directory: String,
    /// Files actually removed, including when a later entry failed.
    pub files: i64,
    /// Bytes actually freed by those confirmed removals.
    pub bytes: i64,
    /// An explicit refusal or failure for the rest of this directory.
    pub error: Option<String>,
}

/// An inspected immutable prefix strictly before one retained native user input.
/// This is not an ordinal or an instruction to hide later displayed messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeInputPrefix {
    /// The adapter that verified the native representation.
    pub harness_kind: String,
    /// The source lane as it was inspected.
    pub source: NativePrefixSource,
    /// The resolved retained native item/client identity, never a display ordinal
    /// or a reference inferred by matching its text.
    pub before_input_ref: String,
    /// Adapter-versioned fingerprint of the exact native checkpoint, including
    /// every retained inherited range and its identity; opening verifies it again.
    pub fingerprint: Hash,
    /// No native conversation/context precedes this boundary; opening creates
    /// an empty session instead of rounding a nonempty history to a turn.
    pub empty: bool,
}

/// A fresh native conversation prepared from the exact inspected prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativePrefixOpened {
    /// The distinct native session; preparation has submitted no model input.
    pub session: crate::session::SessionOpened,
    /// The exact token applied, without rounding to a coarser turn boundary.
    pub prefix: NativeInputPrefix,
}

/// Which archive a session is exported as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    /// The harness's own record.
    Native,
    /// The conversation as items.
    Neutral,
}

/// A session exported, as a file the adapter wrote and the connector uploads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionArchive {
    /// The format: the harness's own name for it, or [`NEUTRAL_FORMAT`].
    pub format: String,
    /// The file the adapter wrote. Binary content is never sent inline: the connector uploads this file
    /// and names it by its hash.
    pub path: String,
    /// Its size in bytes.
    pub size: i64,
}

/// An archive as it is named on the wire: by the hash of its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveRef {
    /// The format.
    pub format: String,
    /// The content's hash.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// A neutral history as it is named on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryRef {
    /// The content's hash.
    pub blob: Hash,
    /// Its size in bytes.
    pub size: i64,
}

/// How faithfully a history arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    /// The whole record, as the harness holds it.
    Lossless,
    /// The conversation as items: its steps are written out, and not what the harness kept inside.
    Structured,
    /// A summary written for the new side.
    Summary,
}

/// What became of the items of one kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KindCounts {
    /// Taken over as they were.
    pub kept: i64,
    /// Taken over in another form.
    pub converted: i64,
    /// Left behind.
    pub dropped: i64,
}

/// A kind of item that was dropped, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Dropped {
    /// The kind.
    pub kind: String,
    /// How many.
    pub count: i64,
    /// Why, in a sentence.
    pub reason: String,
}

/// What an import or a fork did with the items of a history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryReport {
    /// How faithfully it arrived.
    pub fidelity: Fidelity,
    /// How many items there were.
    pub items_total: i64,
    /// What became of each kind of item.
    pub by_kind: BTreeMap<String, KindCounts>,
    /// What was dropped and why.
    pub dropped: Vec<Dropped>,
    /// Things worth knowing that are not losses.
    pub warnings: Vec<String>,
}

/// Builds a [`HistoryReport`] as items are carried over one by one.
#[derive(Debug, Clone, Default)]
pub struct ReportBuilder {
    by_kind: BTreeMap<String, KindCounts>,
    reasons: BTreeMap<(String, String), i64>,
    warnings: Vec<String>,
}

impl ReportBuilder {
    /// An empty report.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An item of `kind` was taken over as it was.
    pub fn keep(&mut self, kind: &str) {
        self.by_kind.entry(kind.to_owned()).or_default().kept += 1;
    }

    /// Adds a known aggregate without visiting every item or creating an empty kind.
    pub fn keep_many(&mut self, kind: &str, count: i64) {
        if count > 0 {
            self.by_kind.entry(kind.to_owned()).or_default().kept += count;
        }
    }

    /// An item of `kind` was taken over in another form.
    pub fn convert(&mut self, kind: &str) {
        self.by_kind.entry(kind.to_owned()).or_default().converted += 1;
    }

    /// An item of `kind` was left behind, for `reason`.
    pub fn drop_item(&mut self, kind: &str, reason: &str) {
        self.drop_many(kind, reason, 1);
    }

    /// Adds an already counted loss without visiting each missing item.
    pub fn drop_many(&mut self, kind: &str, reason: &str, count: i64) {
        if count > 0 {
            self.by_kind.entry(kind.to_owned()).or_default().dropped += count;
            *self
                .reasons
                .entry((kind.to_owned(), reason.to_owned()))
                .or_default() += count;
        }
    }

    /// Something worth knowing that is not a loss.
    pub fn warn(&mut self, warning: &str) {
        if !self.warnings.iter().any(|known| known == warning) {
            self.warnings.push(warning.to_owned());
        }
    }

    /// The report: the items are counted from what was recorded, and the drops are listed by kind and
    /// reason.
    #[must_use]
    pub fn finish(self, fidelity: Fidelity) -> HistoryReport {
        let items_total = self
            .by_kind
            .values()
            .map(|counts| counts.kept + counts.converted + counts.dropped)
            .sum();
        HistoryReport {
            fidelity,
            items_total,
            by_kind: self.by_kind,
            dropped: self
                .reasons
                .into_iter()
                .map(|((kind, reason), count)| Dropped {
                    kind,
                    count,
                    reason,
                })
                .collect(),
            warnings: self.warnings,
        }
    }
}

/// What a request does with the history of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMode {
    /// All of it.
    All,
    /// A range of turns.
    Range,
    /// A summary written for the new side.
    Summary,
    /// None: the new session knows nothing of the source.
    None,
}

/// The ways a history can be carried to another harness process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryMethod {
    /// The session continues: the same provider and profile, and the harness stays where it is.
    SameSession,
    /// The adapter forks the session at that turn.
    NativeFork,
    /// The native archive of the source is imported by the target.
    NativeTransplant,
    /// The turns asked for are imported as items.
    NeutralImport,
    /// The full conversation is materialized as a file the new side reads.
    FileHandoff,
    /// A summary is written for the new side.
    SummaryHandoff,
    /// A new session that knows nothing of the source.
    None,
}

impl HistoryMethod {
    /// How faithful the history is that this method carries; `None` for the method that carries none.
    #[must_use]
    pub const fn fidelity(self) -> Option<Fidelity> {
        match self {
            Self::SameSession | Self::NativeFork | Self::NativeTransplant => {
                Some(Fidelity::Lossless)
            }
            Self::NeutralImport | Self::FileHandoff => Some(Fidelity::Structured),
            Self::SummaryHandoff => Some(Fidelity::Summary),
            Self::None => None,
        }
    }
}

/// What a fork or a switch asks for, as far as the choice of method needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryRequest {
    /// What the request does with the history.
    pub mode: HistoryMode,
    /// Whether it is a switch of a thread (and not a fork).
    pub is_switch: bool,
    /// Whether the target is the harness process of the source: the same provider and profile, with the
    /// harness staying where it is (only the environment or the executor changes).
    pub same_harness_process: bool,
    /// Whether the history asked for starts at the first turn.
    pub from_first_turn: bool,
    /// Whether the two profiles are of one harness kind.
    pub same_kind: bool,
    /// Whether the source's harness can export its native session now.
    pub source_reachable: bool,
}

/// The method for a request, given the capabilities of the source's adapter and of the target's: the first
/// row that applies and that the adapter accepts. `declined` lists the methods an adapter has answered
/// `unsupported` to for this request though it declares them in general (a native fork at an earlier turn, a
/// transcript from a version it does not read); the next row is then tried.
#[must_use]
pub fn choose_method(
    request: &HistoryRequest,
    source: &Capabilities,
    target: &Capabilities,
    declined: &[HistoryMethod],
) -> HistoryMethod {
    use HistoryMethod::{
        FileHandoff, NativeFork, NativeTransplant, NeutralImport, SameSession, SummaryHandoff,
    };
    let candidates: [(HistoryMethod, bool); 4] = [
        (
            SameSession,
            request.is_switch && request.same_harness_process && request.mode == HistoryMode::All,
        ),
        (
            NativeFork,
            !request.is_switch
                && request.same_harness_process
                && request.from_first_turn
                && source.fork_native,
        ),
        (
            NativeTransplant,
            request.same_kind
                && source.export_native
                && target.import_native
                && request.source_reachable
                && request.from_first_turn,
        ),
        (NeutralImport, target.import_neutral),
    ];
    match request.mode {
        HistoryMode::None => HistoryMethod::None,
        HistoryMode::Summary => SummaryHandoff,
        HistoryMode::All | HistoryMode::Range => candidates
            .into_iter()
            .find(|(method, applies)| *applies && !declined.contains(method))
            .map_or(FileHandoff, |(method, _)| method),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_native_identity_alone_grants_no_history_custody_and_unknown_sizes_stay_unknown() {
        let source = serde_json::json!({"thread_id":"00000000-0000-4000-8000-000000000001","segment_id":"00000000-0000-4000-8000-000000000002","native_session_ref":"native-thread"});
        let request = serde_json::json!({"source":source,"started_by_orch":true});
        let read: NativeHistorySource = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), request);
        assert!(!NativeHistorySource::from(read.source.clone()).started_by_orch);
        let fact = serde_json::json!({"source":source,"bytes":null,"removed":false,"error":"history.not_owned"});
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<NativeHistoryFact>(fact.clone()).unwrap()
            )
            .unwrap(),
            fact
        );
    }
    use serde_json::json;

    fn request() -> HistoryRequest {
        HistoryRequest {
            mode: HistoryMode::All,
            is_switch: false,
            same_harness_process: false,
            from_first_turn: true,
            same_kind: true,
            source_reachable: true,
        }
    }

    fn can(export: bool, import: bool, neutral: bool, fork: bool) -> Capabilities {
        Capabilities {
            export_native: export,
            import_native: import,
            import_neutral: neutral,
            fork_native: fork,
            ..Capabilities::default()
        }
    }

    #[test]
    fn a_report_counts_what_was_kept_converted_and_dropped_and_why() {
        let mut report = ReportBuilder::new();
        report.keep("user_message");
        report.keep("user_message");
        report.convert("tool_call");
        report.drop_item("reasoning", "the upstream rejects stored hidden reasoning");
        report.drop_item("reasoning", "the upstream rejects stored hidden reasoning");
        report.drop_item("marker", "markers are not history");
        report.warn("the first 3 turns were left out");
        report.warn("the first 3 turns were left out");
        let report = report.finish(Fidelity::Structured);
        assert_eq!(report.items_total, 6);
        assert_eq!(report.by_kind["user_message"].kept, 2);
        assert_eq!(report.by_kind["reasoning"].dropped, 2);
        assert_eq!(report.dropped.len(), 2);
        assert_eq!(
            report.dropped[0],
            Dropped {
                kind: "marker".into(),
                count: 1,
                reason: "markers are not history".into(),
            }
        );
        assert_eq!(report.warnings.len(), 1, "a warning is told once");
        let written = serde_json::to_value(&report).unwrap();
        assert_eq!(written["fidelity"], "structured");
        assert_eq!(
            written["by_kind"]["tool_call"],
            json!({ "kept": 0, "converted": 1, "dropped": 0 })
        );
    }

    #[test]
    fn a_report_reads_as_the_api_shows_it() {
        let report: HistoryReport = serde_json::from_value(json!({
            "fidelity": "lossless", "items_total": 12,
            "by_kind": { "agent_message": { "kept": 12, "converted": 0, "dropped": 0 } },
            "dropped": [], "warnings": []
        }))
        .unwrap();
        assert_eq!(report.fidelity, Fidelity::Lossless);
    }

    #[test]
    fn a_request_for_no_history_or_a_summary_decides_the_method_alone() {
        let any = can(true, true, true, true);
        let mut none = request();
        none.mode = HistoryMode::None;
        assert_eq!(choose_method(&none, &any, &any, &[]), HistoryMethod::None);
        let mut summary = request();
        summary.mode = HistoryMode::Summary;
        assert_eq!(
            choose_method(&summary, &any, &any, &[]),
            HistoryMethod::SummaryHandoff
        );
        assert_eq!(HistoryMethod::None.fidelity(), None);
        assert_eq!(
            HistoryMethod::SummaryHandoff.fidelity(),
            Some(Fidelity::Summary)
        );
    }

    #[test]
    fn a_switch_inside_one_harness_process_keeps_the_session() {
        let mut switch = request();
        switch.is_switch = true;
        switch.same_harness_process = true;
        let nothing = Capabilities::default();
        assert_eq!(
            choose_method(&switch, &nothing, &nothing, &[]),
            HistoryMethod::SameSession
        );
        assert_eq!(
            HistoryMethod::SameSession.fidelity(),
            Some(Fidelity::Lossless)
        );
    }

    #[test]
    fn a_fork_inside_one_process_forks_natively_only_from_the_first_turn_and_where_it_is_declared()
    {
        let mut fork = request();
        fork.same_harness_process = true;
        let can_fork = can(true, true, true, true);
        assert_eq!(
            choose_method(&fork, &can_fork, &can_fork, &[]),
            HistoryMethod::NativeFork
        );
        fork.from_first_turn = false;
        assert_eq!(
            choose_method(&fork, &can_fork, &can_fork, &[]),
            HistoryMethod::NeutralImport,
            "a native fork or a transplant needs the history to start at the first turn"
        );
        fork.from_first_turn = true;
        assert_eq!(
            choose_method(&fork, &can_fork, &can_fork, &[HistoryMethod::NativeFork]),
            HistoryMethod::NativeTransplant,
            "the adapter answered unsupported, so the next row is tried"
        );
    }

    #[test]
    fn a_transplant_needs_one_kind_of_harness_a_reachable_source_and_both_ends_declared() {
        let both = can(true, true, false, false);
        let mut transplant = request();
        assert_eq!(
            choose_method(&transplant, &both, &both, &[]),
            HistoryMethod::NativeTransplant
        );
        transplant.source_reachable = false;
        assert_eq!(
            choose_method(&transplant, &both, &both, &[]),
            HistoryMethod::FileHandoff
        );
        transplant.source_reachable = true;
        transplant.same_kind = false;
        assert_eq!(
            choose_method(&transplant, &both, &both, &[]),
            HistoryMethod::FileHandoff
        );
        let neutral = can(false, false, true, false);
        assert_eq!(
            choose_method(&transplant, &both, &neutral, &[]),
            HistoryMethod::NeutralImport
        );
        assert_eq!(
            HistoryMethod::NeutralImport.fidelity(),
            Some(Fidelity::Structured)
        );
    }
}
