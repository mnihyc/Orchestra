//! Factual source roles for the native Skill creator's closed source intents.
use crate::operation_sources::{Binding, Cardinality, Declaration};
use crate::operations::{FacetSpec, FacetUsage, TargetResolver};
use orch_access_contract::{
    key_scope::{KeyAction, KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};

/// One real native home, source or completed Skill; a hash never stands for an Upload identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The actual existing Org or Project selected by native making.
    Home,
    /// The acting Account's actual retained upload custody.
    Upload,
    /// The actual existing Environment supplying its managed directory.
    Environment,
    /// The actual existing Host supplying a native repository or URL fetch.
    Host,
    /// The actual completed stored Skill, independently readable after creation.
    Created,
    References,
    Groups,
    Orgs,
    Projects,
    Topics,
    Threads,
}
const HOME: TargetResolver = TargetResolver::Resolved("home");
const UPLOAD: TargetResolver = TargetResolver::Resolved("source_upload");
const ENVIRONMENT: TargetResolver = TargetResolver::Resolved("source_environment");
const HOST: TargetResolver = TargetResolver::Resolved("fetch_host");
const CREATED: TargetResolver = TargetResolver::Resolved("created_skill");
const HOME_FACET: FacetSpec = FacetSpec::required(
    KeyAction::Manage,
    &[KeyTargetKind::Object(ObjectKind::Skill)],
    HOME,
);
const CREATED_FACET: FacetSpec =
    FacetSpec::response_content(&[KeyTargetKind::Object(ObjectKind::Skill)], CREATED);
/// The supplied new draft reads no stored upload, Environment or Host.
pub const NEW_FACETS: [FacetSpec; 8] = [
    HOME_FACET,
    CREATED_FACET,
    RESPONSE[0],
    RESPONSE[1],
    RESPONSE[2],
    RESPONSE[3],
    RESPONSE[4],
    RESPONSE[5],
];
/// The selected retained upload is governed by its actual acting Account.
pub const UPLOAD_FACETS: [FacetSpec; 9] = [
    HOME_FACET,
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Identity(KeyIdentityKind::Upload)],
        UPLOAD,
    ),
    CREATED_FACET,
    RESPONSE[0],
    RESPONSE[1],
    RESPONSE[2],
    RESPONSE[3],
    RESPONSE[4],
    RESPONSE[5],
];
/// Native directory import keeps both of its original source actions under one factual target.
pub const ENVIRONMENT_FACETS: [FacetSpec; 10] = [
    HOME_FACET,
    FacetSpec::required(
        KeyAction::Read,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        ENVIRONMENT,
    ),
    FacetSpec::required(
        KeyAction::Manage,
        &[KeyTargetKind::Object(ObjectKind::Environment)],
        ENVIRONMENT,
    ),
    CREATED_FACET,
    RESPONSE[0],
    RESPONSE[1],
    RESPONSE[2],
    RESPONSE[3],
    RESPONSE[4],
    RESPONSE[5],
];
/// Repository fetch keeps its native Host Write and independent completed Skill Read.
pub const GIT_FACETS: [FacetSpec; 9] = [
    HOME_FACET,
    FacetSpec::required(
        KeyAction::Write,
        &[KeyTargetKind::Object(ObjectKind::Host)],
        HOST,
    ),
    CREATED_FACET,
    RESPONSE[0],
    RESPONSE[1],
    RESPONSE[2],
    RESPONSE[3],
    RESPONSE[4],
    RESPONSE[5],
];
/// URL fetch uses the same real Host role; its private source setup remains a separate native cut.
pub const URL_FACETS: [FacetSpec; 9] = GIT_FACETS;
const HOME_BINDING: Binding<Source> = Binding {
    source: Source::Home,
    target: HOME,
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const CREATED_BINDING: Binding<Source> = Binding {
    source: Source::Created,
    target: CREATED,
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};
pub const REFERENCES: &str = "skill_created_sources";
pub const GROUPS: &str = "skill_created_groups";
pub const NODES: [&str; 4] = [
    "skill_created_orgs",
    "skill_created_projects",
    "skill_created_topics",
    "skill_created_threads",
];
const RESPONSE: [FacetSpec; 6] = [
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Skill)],
        TargetResolver::ResolvedCollection(REFERENCES),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Identity(KeyIdentityKind::Group)],
        TargetResolver::ResolvedCollection(GROUPS),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Org)],
        TargetResolver::ResolvedCollection(NODES[0]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Project)],
        TargetResolver::ResolvedCollection(NODES[1]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Topic)],
        TargetResolver::ResolvedCollection(NODES[2]),
    ),
    FacetSpec::response_content(
        &[KeyTargetKind::Object(ObjectKind::Thread)],
        TargetResolver::ResolvedCollection(NODES[3]),
    ),
];
const RESPONSE_BINDINGS: [Binding<Source>; 6] = [
    Binding {
        source: Source::References,
        target: TargetResolver::ResolvedCollection(REFERENCES),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Groups,
        target: TargetResolver::ResolvedCollection(GROUPS),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Orgs,
        target: TargetResolver::ResolvedCollection(NODES[0]),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Projects,
        target: TargetResolver::ResolvedCollection(NODES[1]),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Topics,
        target: TargetResolver::ResolvedCollection(NODES[2]),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
    Binding {
        source: Source::Threads,
        target: TargetResolver::ResolvedCollection(NODES[3]),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    },
];
/// Pure new-draft source row; no missing source is fabricated as an empty singleton.
pub const NEW: Declaration<Source> = Declaration {
    operation: "SkillCreate.new",
    bindings: &[
        HOME_BINDING,
        CREATED_BINDING,
        RESPONSE_BINDINGS[0],
        RESPONSE_BINDINGS[1],
        RESPONSE_BINDINGS[2],
        RESPONSE_BINDINGS[3],
        RESPONSE_BINDINGS[4],
        RESPONSE_BINDINGS[5],
    ],
};
/// Actual completed-upload custody, never content-hash ancestry.
pub const UPLOAD_ROW: Declaration<Source> = Declaration {
    operation: "SkillCreate.upload",
    bindings: &[
        HOME_BINDING,
        Binding {
            source: Source::Upload,
            target: UPLOAD,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CREATED_BINDING,
        RESPONSE_BINDINGS[0],
        RESPONSE_BINDINGS[1],
        RESPONSE_BINDINGS[2],
        RESPONSE_BINDINGS[3],
        RESPONSE_BINDINGS[4],
        RESPONSE_BINDINGS[5],
    ],
};
/// Actual native directory source supplies both required actions declared by the canonical variant.
pub const ENVIRONMENT_ROW: Declaration<Source> = Declaration {
    operation: "SkillCreate.environment",
    bindings: &[
        HOME_BINDING,
        Binding {
            source: Source::Environment,
            target: ENVIRONMENT,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CREATED_BINDING,
        RESPONSE_BINDINGS[0],
        RESPONSE_BINDINGS[1],
        RESPONSE_BINDINGS[2],
        RESPONSE_BINDINGS[3],
        RESPONSE_BINDINGS[4],
        RESPONSE_BINDINGS[5],
    ],
};
/// Actual native repository fetch Host.
pub const GIT: Declaration<Source> = Declaration {
    operation: "SkillCreate.git",
    bindings: &[
        HOME_BINDING,
        Binding {
            source: Source::Host,
            target: HOST,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CREATED_BINDING,
        RESPONSE_BINDINGS[0],
        RESPONSE_BINDINGS[1],
        RESPONSE_BINDINGS[2],
        RESPONSE_BINDINGS[3],
        RESPONSE_BINDINGS[4],
        RESPONSE_BINDINGS[5],
    ],
};
/// Actual native URL fetch Host; no unrelated Environment or Upload role is invented.
pub const URL: Declaration<Source> = Declaration {
    operation: "SkillCreate.url",
    bindings: &[
        HOME_BINDING,
        Binding {
            source: Source::Host,
            target: HOST,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        CREATED_BINDING,
        RESPONSE_BINDINGS[0],
        RESPONSE_BINDINGS[1],
        RESPONSE_BINDINGS[2],
        RESPONSE_BINDINGS[3],
        RESPONSE_BINDINGS[4],
        RESPONSE_BINDINGS[5],
    ],
};
