//! Shared summary targets, target options and bounded read/refresh projections.
use crate::common::{Problem, Revision};
use orch_provider_contract::capabilities::{ProviderView, Resolution};
use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

/// Every place a summary paragraph is shown, independent of its producer.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SummaryKind {
    /// An agent conversation.
    Thread,
    /// Related work at a topic.
    Topic,
    /// One ordinary document.
    Document,
    /// A viewer's unread group/node chat messages.
    Chat,
    /// A cached chat subject, independent of the reader's unread range.
    ChatTitle,
    /// An authored guide, read from this build.
    Help,
    /// Readable hits' existing summaries.
    SearchResults,
}

/// The normal events a target can use; no event starts work unless automatic refresh is enabled.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SummaryTrigger {
    /// A completed agent turn.
    TurnCompleted,
    /// A reported milestone.
    Milestone,
    /// Periodic refresh selected by the place.
    Interval,
    /// Document content changed.
    ContentChanged,
    /// A chat message arrived.
    MessagePosted,
    /// A readable child of a topic changed.
    ChildChanged,
}

/// The same automatic summary options for every generated target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SummaryTargetOptions {
    /// Off as shipped; explicit refresh is a separate person-issued act.
    pub automatic: bool,
    /// Omitted uses the target's ordinary change events; empty disables automatic event triggers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<BTreeSet<SummaryTrigger>>,
    /// Overrides the shared automatic pacing; zero adds no minimum gap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_interval_secs: Option<u32>,
    /// Positive input token budget, overriding the producer's 32000-token default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<NonZeroU32>,
    /// Positive output characters; a default/budget, never a provider or usage-cap override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_chars: Option<NonZeroU32>,
}

/// Written/cached targets have no model-generation knobs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WrittenSummaryOptions {}

/// One setting family, with independent values for each target rather than per-program keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SummaryTargets {
    /// Thread defaults, overridable at a single thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<SummaryTargetOptions>,
    /// Topic defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<SummaryTargetOptions>,
    /// Defaults for documents; an individual document may choose its own options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<SummaryTargetOptions>,
    /// Chat/catch-up defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<SummaryTargetOptions>,
    /// Chat title defaults, independent of catch-up options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_title: Option<SummaryTargetOptions>,
    /// Authored help; never a model invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<WrittenSummaryOptions>,
    /// Existing result summaries; never an implicit model invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_results: Option<WrittenSummaryOptions>,
}

impl SummaryTargets {
    /// Generated-target options only. Written/cached views cannot be made into a paid job here.
    #[must_use]
    pub const fn generated(&self, kind: SummaryKind) -> Option<&SummaryTargetOptions> {
        match kind {
            SummaryKind::Thread => self.thread.as_ref(),
            SummaryKind::Topic => self.topic.as_ref(),
            SummaryKind::Document => self.document.as_ref(),
            SummaryKind::Chat => self.chat.as_ref(),
            SummaryKind::ChatTitle => self.chat_title.as_ref(),
            SummaryKind::Help | SummaryKind::SearchResults => None,
        }
    }
}

/// Persisted resources eligible for the ordinary summary API; static guides stay in the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SummaryTarget {
    /// A thread, governed by its current View.
    Thread {
        /// Its existing node identity.
        id: Id,
    },
    /// A topic, governed by its current View.
    Topic {
        /// Its existing node identity.
        id: Id,
    },
    /// One document, governed by current document View.
    Document {
        /// Its existing document identity.
        id: Id,
    },
    /// A chat, with current visibility and a viewer-specific catch-up cursor.
    Chat {
        /// Its existing chat identity.
        id: Id,
    },
    /// A separately cached chat title, governed by that same actual chat.
    ChatTitle {
        /// Its existing chat identity.
        id: Id,
    },
}

impl SummaryTarget {
    /// The resource's existing identity, never a guessed hierarchy parent.
    #[must_use]
    pub const fn id(self) -> Id {
        match self {
            Self::Thread { id }
            | Self::Topic { id }
            | Self::Document { id }
            | Self::Chat { id }
            | Self::ChatTitle { id } => id,
        }
    }

    /// The corresponding setting-family entry.
    #[must_use]
    pub const fn kind(self) -> SummaryKind {
        match self {
            Self::Thread { .. } => SummaryKind::Thread,
            Self::Topic { .. } => SummaryKind::Topic,
            Self::Document { .. } => SummaryKind::Document,
            Self::Chat { .. } => SummaryKind::Chat,
            Self::ChatTitle { .. } => SummaryKind::ChatTitle,
        }
    }
}

/// A person's optional API model choice, checked against current Use and allowed lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryModelChoice {
    /// Actual provider identity.
    pub provider_id: Id,
    /// Its exact enabled model.
    pub model: String,
    /// Provider-bounded options in that model's existing interface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Json>,
}

/// One reader's pinned catch-up selection: messages created in `(after, through]`.
/// A range is never a permission or a read mark. The native chat producer validates its
/// nonnegative order and current endpoint before selecting any message content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryChatRange {
    /// The reader's captured last-read position, excluded.
    pub after: i64,
    /// The captured message endpoint, included.
    pub through: i64,
}

/// An explicit request. Payer/requester authority is never taken from its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryRefreshBody {
    /// Stable across a retry of this one act.
    pub client_id: Id,
    /// A personal choice; omitted uses the current target/place choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<SummaryModelChoice>,
    /// A chat page pins this before its ordinary message read-mark act. Other targets reject it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<SummaryChatRange>,
}

/// An individual document's own options. Null restores the place default; node/thread
/// entries stay in the ordinary scoped summary.targets family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryOptionsBody {
    /// Own options, or inheritance.
    pub options: Option<SummaryTargetOptions>,
    /// The options revision the caller read.
    pub expected_revision: Revision,
}

/// One document's effective target options and its own override. Producer configuration,
/// ancestor names and paying identities are not part of this ordinary document projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryOptionsView {
    /// The actual native document.
    pub target: SummaryTarget,
    /// The effective document entry after the ordinary ancestor locks.
    pub options: SummaryTargetOptions,
    /// Effective pacing, including the kind default; separate from an optional stored override.
    pub min_interval_secs: u32,
    /// This document's stored override, or inheritance.
    pub own: Option<SummaryTargetOptions>,
    /// Own-options CAS revision, independent of content or paragraph revisions.
    pub revision: Revision,
    /// Current ordinary document edit permission and no inherited lock.
    pub writable: bool,
    /// An existing ancestor lock decides the options here.
    pub locked: bool,
}

/// Safe acknowledgement of a document options write, without current stored options/content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryOptionsReceipt {
    /// This response carries mutation metadata only.
    pub projection: crate::common::ReceiptProjection,
    /// The already admitted actual document.
    pub target: SummaryTarget,
    /// The resulting own-options revision.
    pub revision: Revision,
}

/// An optional pinned chat range; both endpoints must be present together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryReadQuery {
    /// Last-read message sequence, excluded.
    pub after: Option<i64>,
    /// Captured current message sequence, included.
    pub through: Option<i64>,
}

/// Actual native source families, selected before the common operation gate.
pub const SUMMARY_FAMILIES: &[orch_access_contract::key_scope::KeyTargetKind] = &[
    orch_access_contract::key_scope::KeyTargetKind::Object(
        orch_access_contract::permissions::ObjectKind::Thread,
    ),
    orch_access_contract::key_scope::KeyTargetKind::Object(
        orch_access_contract::permissions::ObjectKind::Topic,
    ),
    orch_access_contract::key_scope::KeyTargetKind::Object(
        orch_access_contract::permissions::ObjectKind::Document,
    ),
    orch_access_contract::key_scope::KeyTargetKind::Identity(
        orch_access_contract::key_scope::KeyIdentityKind::Chat,
    ),
];

/// Reading a paragraph reads all its actual governors. A paid refresh writes its
/// actual resource cache; this Write facet keeps paid work out of read-only tool selection.
pub const fn summary_facets(refresh: bool) -> [crate::operations::FacetSpec; 3] {
    use crate::operations::{FacetSpec as F, TargetResolver as T};
    use orch_access_contract::{
        key_scope::{KeyAction as A, KeyTargetKind as K},
        permissions::ObjectKind as O,
    };
    [
        F::required(A::Read, SUMMARY_FAMILIES, T::Resolved("summary_target")),
        F::required(
            A::Read,
            SUMMARY_FAMILIES,
            T::ResolvedCollection("summary_inputs"),
        ),
        if refresh {
            F::required(A::Write, SUMMARY_FAMILIES, T::Resolved("summary_target"))
        } else {
            F::response_content(
                &[K::Object(O::Provider)],
                T::ResolvedCollection("summary_providers"),
            )
        },
    ]
}

crate::endpoints! {
    /// Reads stored paragraphs in a bounded source window; never creates or refreshes summaries.
    SummaryBatchGet: POST "/summaries/batch", crate::routes::Access::Rule("current native source Read for each returned target and every paragraph governor"), none, json SummaryBatchBody, json SummaryBatchReply, 200,
        operation crate::operations::OperationSpec::facets(&summary_batch_facets());
    /// Reads the current reader's paragraph without creating a cache or starting paid work.
    SummaryGet: GET "/summaries/{kind}/{id}", crate::routes::Access::Rule("current native source Read for every included contribution"), query SummaryReadQuery, none, json SummarySnapshot, 200,
        operation crate::operations::OperationSpec::facets(&summary_facets(false));
    /// Accepts one original person-issued summary refresh with current source Read and provider Use.
    SummaryRefresh: POST "/summaries/{kind}/{id}/refresh", crate::routes::Access::Rule("current source Read, provider Use, allowed model and person summary caps"), none, json SummaryRefreshBody, json SummaryReceipt, 202,
        operation crate::operations::OperationSpec::receipt(&crate::operations::OperationSpec::facets(&summary_facets(true)));
    /// Reads one document's summary options, preserving its actual document permission and ancestry.
    DocumentSummaryOptionsGet: GET "/documents/{id}/summary-options", crate::routes::Access::Rule("doc.view on the actual document"), none, none, json SummaryOptionsView, 200,
        operation crate::operations::OperationSpec::facets(&[
            crate::operations::FacetSpec::required(
                orch_access_contract::key_scope::KeyAction::Read,
                &[orch_access_contract::key_scope::KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Document)],
                crate::operations::TargetResolver::TypedPath { parameter:"id", kind:orch_access_contract::key_scope::KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Document) },
            ),
        ]);
    /// Sets or inherits one document's summary options; this creates no model request or paragraph.
    DocumentSummaryOptionsSet: PUT "/documents/{id}/summary-options", crate::routes::Access::Rule("doc.edit on the actual document and no inherited summary-options lock"), none, json SummaryOptionsBody, json SummaryOptionsReceipt, 200,
        operation crate::operations::OperationSpec::receipt(&crate::operations::OperationSpec::facets(&[
            crate::operations::FacetSpec::required(
                orch_access_contract::key_scope::KeyAction::Write,
                &[orch_access_contract::key_scope::KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Document)],
                crate::operations::TargetResolver::TypedPath { parameter:"id", kind:orch_access_contract::key_scope::KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Document) },
            ),
        ]));
}

/// Batch selection hints, never a cached read authority or a model request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryBatchBody {
    /// Native resource identities, in caller order; repeated identities are read once.
    pub targets: Vec<SummaryTarget>,
    /// Offset in distinct caller targets; later windows repeat the same current checks.
    #[serde(default)]
    pub offset: u32,
    /// Include the same complete current state and controls as the singleton summary reader.
    #[serde(default)]
    pub full: bool,
    /// Explicit chat opening ranges, keyed by their actual chat identity.
    #[serde(default)]
    pub ranges: BTreeMap<Id, SummaryChatRange>,
}

/// Minimal stored paragraph projection for a search result and its unsent Ask draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryProjection {
    /// Actual readable source.
    pub target: SummaryTarget,
    /// Actual native root relationship; not root Read or a model admission.
    pub root_org_id: Id,
    /// At least one disclosed governor is passive inspection only.
    pub inspecting: bool,
    /// Existing readable paragraph, or no paragraph for this currently readable source.
    pub text: Option<String>,
    /// Stored producer facts; provider and offering remain subject to optional current provider Read.
    pub producer: Option<SummaryProducerView>,
    /// Actual last successful publication, if any.
    pub updated_at: Option<Time>,
    /// Complete native view when explicitly requested, with all singleton read checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<SummarySnapshot>,
}

/// One bounded native read; unavailable identities reveal only the caller's own supplied ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryBatchReply {
    /// Stored paragraphs after complete current source checks.
    pub items: Vec<SummaryProjection>,
    /// Supplied targets currently missing or outside this operation's current Read.
    pub unavailable: Vec<SummaryTarget>,
    /// More distinct targets remain; this is a selection offset, not a capability.
    pub next_offset: Option<u32>,
}

/// Batch target/source facts use their own operation namespace and collection cardinality.
pub const fn summary_batch_facets() -> [crate::operations::FacetSpec; 3] {
    use crate::operations::{FacetSpec as F, TargetResolver as T};
    use orch_access_contract::key_scope::KeyAction as A;
    [
        F::required(
            A::Read,
            SUMMARY_FAMILIES,
            T::ResolvedCollection("summary_targets"),
        ),
        F::required(
            A::Read,
            SUMMARY_FAMILIES,
            T::ResolvedCollection("summary_inputs"),
        ),
        F::response_content(
            &[orch_access_contract::key_scope::KeyTargetKind::Object(
                orch_access_contract::permissions::ObjectKind::Provider,
            )],
            T::ResolvedCollection("summary_providers"),
        ),
    ]
}

/// A safe mutation receipt, separate from reading the paragraph or any source material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryReceipt {
    /// Mutation metadata only; reading text is a separate current-authority operation.
    pub projection: crate::common::ReceiptProjection,
    /// The existing summary/job identity.
    pub id: Id,
    /// The already admitted resource identity.
    pub target: SummaryTarget,
    /// Its current summary revision.
    pub revision: Revision,
}

/// Factual state, rather than a timestamp guessed fresh by the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryState {
    /// No paragraph has been produced yet.
    Empty,
    /// The paragraph matches its captured current source.
    Fresh,
    /// Source changed without a queued automatic update.
    Stale,
    /// An accepted automatic or recovery attempt has a known future eligibility time.
    Scheduled,
    /// A real queued/running attempt exists.
    Updating,
    /// The last attempt failed; previous text remains readable.
    Failed,
    /// No producer is selected or available.
    NoSummarizer,
}

/// A readable producer identity, with private provider/offering details omitted independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryProducerView {
    /// Installed producer kind, independent of a vendor.
    pub kind: String,
    /// Its supplied human name.
    pub title: String,
    /// Only when current Read permits this provider's identity.
    pub provider: Option<ProviderView>,
    /// Only when current Read permits this offering's identity.
    pub offering: Option<String>,
}

/// Why no default model is available for a person's explicit refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryManualProblem {
    /// No available API model passes current Use and the place's allowlists.
    NoModel,
    /// An otherwise usable model has no room under the person's summary caps.
    CapReached,
}

/// The native reason an accepted update is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SummaryWaitReason {
    /// The target's configured minimum interval has not elapsed.
    Interval,
    /// An existing call rule reserved a later retry.
    Recovery,
    /// Eligible work is waiting for its native worker.
    Queue,
}

/// Factual wait state, independent of freshness or a guessed browser timer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryWait {
    /// Actual native wait cause.
    pub reason: SummaryWaitReason,
    /// Earliest known eligibility; absent for ordinary queued work.
    pub until: Option<Time>,
}

/// The ordinary read view of one resource/viewer's summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SummarySnapshot {
    /// Actual native root relationship of this readable source; never organization Read or a name.
    pub root_org_id: Id,
    /// Actual admitted resource.
    pub target: SummaryTarget,
    /// At least one disclosed source is available only through passive inspection.
    /// Such content remains readable but does not become a new model/Ask input act.
    pub inspecting: bool,
    /// Changes with stored text or job state.
    pub revision: Revision,
    /// Current source/job state.
    pub state: SummaryState,
    /// Previous successful text may remain during an update or failure.
    pub text: Option<String>,
    /// Safe producer facts.
    pub producer: Option<SummaryProducerView>,
    /// Coverage of currently readable stored input, without provider/configuration details.
    pub input: Option<orch_plugin_contract::refs::SummaryInputFacts>,
    /// Manual availability; never enables or writes an automatic producer setting.
    pub manual_problem: Option<SummaryManualProblem>,
    /// Actual reason and eligibility time of accepted pending work.
    pub wait: Option<SummaryWait>,
    /// Server-paced next free read; absent when no pending work needs following.
    pub poll_after_ms: Option<u32>,
    /// Last successful publication, absent before the first result.
    pub updated_at: Option<Time>,
    /// The same capability route/quote projection as the inspector, filtered for this reader.
    pub route: Option<Resolution>,
    /// A safe ordinary problem explaining a failed attempt.
    pub problem: Option<Problem>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn batch_read_declares_its_own_collections_and_optional_provider_read_without_paid_write() {
        use crate::routes::Endpoint;
        use orch_access_contract::key_scope::KeyAction;
        let op = crate::operations::for_route(&SummaryBatchGet::ROUTE).unwrap();
        assert_eq!(op.operation_id, "SummaryBatchGet");
        assert!(
            op.variants[0]
                .facets
                .iter()
                .all(|facet| facet.action == KeyAction::Read)
        );
        assert_eq!(op.variants[0].facets.len(), 3);
    }

    #[test]
    fn refresh_is_paid_write_work_and_paragraph_reads_declare_all_native_inputs() {
        use crate::routes::Endpoint;
        use orch_access_contract::key_scope::KeyAction;
        let refresh = crate::operations::for_route(&SummaryRefresh::ROUTE).unwrap();
        let facets = &refresh.variants[0].facets;
        assert_eq!(
            facets
                .iter()
                .filter(|facet| facet.action == KeyAction::Read)
                .count(),
            2
        );
        assert!(facets.iter().any(|facet| {
            facet.action == KeyAction::Write
                && facet.resources.contains(
                    &orch_access_contract::key_scope::KeyTargetKind::Object(
                        orch_access_contract::permissions::ObjectKind::Document,
                    ),
                )
        }));
        let read = crate::operations::for_route(&SummaryGet::ROUTE).unwrap();
        assert!(
            read.variants[0]
                .facets
                .iter()
                .all(|facet| facet.action == KeyAction::Read)
        );
        assert!(read.variants[0].facets.iter().any(|facet| matches!(
            facet.target,
            crate::operations::TargetResolver::ResolvedCollection("summary_inputs")
        ) && facet.usage
            == crate::operations::FacetUsage::Required));
    }

    #[test]
    fn targets_default_to_no_automatic_work_and_static_views_cannot_enable_a_model() {
        let defaults: SummaryTargets = serde_json::from_value(json!({})).unwrap();
        assert_eq!(defaults, SummaryTargets::default());
        assert!(!SummaryTargetOptions::default().automatic);
        for kind in [SummaryKind::Help, SummaryKind::SearchResults] {
            assert!(defaults.generated(kind).is_none());
        }
        for kind in ["help", "search_results"] {
            assert!(
                serde_json::from_value::<SummaryTargets>(json!({(kind):{"automatic":true}}))
                    .is_err()
            );
        }
    }

    #[test]
    fn one_family_accepts_each_target_and_positive_configurable_budgets() {
        let targets: SummaryTargets = serde_json::from_value(json!({
            "thread":{"automatic":true,"output_chars":1200},
            "topic":{"on":["child_changed"]},"document":{"context_tokens":30000},
            "chat":{"on":["message_posted"],"context_tokens":40000},"help":{},"search_results":{}
        }))
        .unwrap();
        assert_eq!(
            targets
                .generated(SummaryKind::Thread)
                .unwrap()
                .output_chars
                .unwrap()
                .get(),
            1200
        );
        for key in ["context_tokens", "output_chars"] {
            assert!(serde_json::from_value::<SummaryTargetOptions>(json!({(key):0})).is_err());
        }
    }

    #[test]
    fn a_refresh_has_an_idempotency_identity_and_cannot_choose_its_responsible_person() {
        let id = orch_types::new_id();
        assert!(serde_json::from_value::<SummaryRefreshBody>(json!({"client_id":id})).is_ok());
        for key in ["payer_id", "requested_by", "user_id", "authority"] {
            assert!(
                serde_json::from_value::<SummaryRefreshBody>(json!({"client_id":id,(key):id}))
                    .is_err()
            );
        }
        assert!(serde_json::from_value::<SummaryRefreshBody>(json!({})).is_err());
    }

    #[test]
    fn document_option_writes_take_no_credential_or_responsibility_and_return_metadata_only() {
        let body = json!({"options":{"automatic":false,"output_chars":1200},"expected_revision":0});
        assert!(serde_json::from_value::<SummaryOptionsBody>(body.clone()).is_ok());
        for field in ["writer_id", "requested_by", "authority", "payer_id"] {
            let mut injected = body.clone();
            injected[field] = json!(orch_types::new_id());
            assert!(serde_json::from_value::<SummaryOptionsBody>(injected).is_err());
        }
        let target = SummaryTarget::Document {
            id: orch_types::new_id(),
        };
        let receipt = SummaryOptionsReceipt {
            projection: crate::common::ReceiptProjection::Receipt,
            target,
            revision: 1,
        };
        let wire = serde_json::to_value(receipt).unwrap();
        assert_eq!(wire.as_object().unwrap().len(), 3);
        assert!(
            wire.get("options").is_none()
                && wire.get("text").is_none()
                && wire.get("producer").is_none()
        );
    }
}
