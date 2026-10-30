//! What a document may hold where it stands, as the settings of its node resolve it, and the refusals that say which limit was met and where it is set.
//!
//! A limit is a setting of the node the document is beside, so that an org, a
//! project or a topic says how large a document, one file and all its files
//! may be. A refusal names the limit, its value and the node whose setting it
//! is, when it is one node's and not the system's or the shipped default,
//! which is what lets a person know whom to ask.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::nodes::view;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    DOCUMENT_BLOCK_MAX_BYTES, DOCUMENT_NESTING_MAX, DOCUMENT_OPS_MAX, DocumentLimits,
};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::queries::documents::batch;
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::Id;

/// A number a setting resolves to, and the node whose row it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// The value.
    pub value: i64,
    /// The node that sets it; none where it is the system's or the registry's.
    pub scope: Option<Id>,
}

/// The `docs.*` settings at a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocSettings {
    /// Whether documents may be made there.
    pub enabled: bool,
    /// The node that switched them off or on.
    pub enabled_scope: Option<Id>,
    /// `docs.max_bytes`.
    pub max_bytes: Setting,
    /// `docs.max_blocks`.
    pub max_blocks: Setting,
    /// `docs.file_max_bytes`.
    pub file_max_bytes: Setting,
    /// `docs.files_max_bytes`.
    pub files_max_bytes: Setting,
    /// `docs.versions_kept`.
    pub versions_kept: Setting,
    /// `docs.history_max_bytes`.
    pub history_max_bytes: Setting,
}

/// Which limit a document met.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The size of all its blocks together.
    Bytes,
    /// The number of its blocks.
    Blocks,
    /// The size of one block.
    BlockBytes,
    /// How deep blocks are nested.
    Depth,
    /// How many operations one batch holds.
    Operations,
    /// The size of one file it holds.
    FileBytes,
    /// The size of all the files it holds together.
    FilesBytes,
}

impl Limit {
    /// The word the refusal names it with.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Bytes => "bytes",
            Self::Blocks => "blocks",
            Self::BlockBytes => "block_bytes",
            Self::Depth => "depth",
            Self::Operations => "operations",
            Self::FileBytes => "file_bytes",
            Self::FilesBytes => "files_bytes",
        }
    }

    /// What it is a limit of, as a sentence says it.
    const fn what(self) -> &'static str {
        match self {
            Self::Bytes => "bytes of text in all",
            Self::Blocks => "blocks",
            Self::BlockBytes => "bytes in one block",
            Self::Depth => "levels of nesting",
            Self::Operations => "operations in one batch",
            Self::FileBytes => "bytes in one file",
            Self::FilesBytes => "bytes of files in all",
        }
    }
}

/// The value of a limit that the contract fixes, and no setting changes.
#[must_use]
pub fn fixed(limit: Limit) -> Option<i64> {
    match limit {
        Limit::BlockBytes => Some(DOCUMENT_BLOCK_MAX_BYTES),
        Limit::Depth => Some(i64::from(DOCUMENT_NESTING_MAX)),
        Limit::Operations => Some(i64::from(DOCUMENT_OPS_MAX)),
        _ => None,
    }
}

impl DocSettings {
    /// Resolves the settings at `node`.
    pub async fn at(conn: &mut Conn, node: Id) -> ApiResult<Self> {
        let wanted = [
            keys::DOCS_ENABLED,
            keys::DOCS_MAX_BYTES,
            keys::DOCS_MAX_BLOCKS,
            keys::DOCS_FILE_MAX_BYTES,
            keys::DOCS_FILES_MAX_BYTES,
            keys::DOCS_VERSIONS_KEPT,
            keys::DOCS_HISTORY_MAX_BYTES,
        ];
        let defs: Vec<&'static registry::SettingDef> =
            wanted.iter().filter_map(|key| registry::get(key)).collect();
        let snapshot = Snapshot::load(conn, Place::Node(node), Wanted::Keys(&defs))
            .await
            .map_err(ApiError::internal)?;
        let resolve = |key: &str| -> ApiResult<orch_settings_contract::provenance::Resolved<Id>> {
            let def = registry::get(key)
                .ok_or_else(|| ApiError::internal(format!("{key} is no setting")))?;
            Ok(snapshot.resolve(def))
        };
        let number = |key: &str| -> ApiResult<Setting> {
            let resolved = resolve(key)?;
            let value = resolved
                .value
                .as_ref()
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| ApiError::internal(format!("{key} resolves to no number")))?;
            Ok(Setting {
                value,
                scope: resolved.source.node,
            })
        };
        let enabled = resolve(keys::DOCS_ENABLED)?;
        Ok(Self {
            enabled: enabled
                .value
                .as_ref()
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            enabled_scope: enabled.source.node,
            max_bytes: number(keys::DOCS_MAX_BYTES)?,
            max_blocks: number(keys::DOCS_MAX_BLOCKS)?,
            file_max_bytes: number(keys::DOCS_FILE_MAX_BYTES)?,
            files_max_bytes: number(keys::DOCS_FILES_MAX_BYTES)?,
            versions_kept: number(keys::DOCS_VERSIONS_KEPT)?,
            history_max_bytes: number(keys::DOCS_HISTORY_MAX_BYTES)?,
        })
    }

    /// What a batch may do to the document.
    #[must_use]
    pub fn batch_limits(&self) -> batch::Limits {
        batch::Limits {
            max_bytes: self.max_bytes.value,
            max_blocks: self.max_blocks.value,
        }
    }

    /// When versions are cut, and how many are kept.
    #[must_use]
    pub fn version_rules(&self, window_secs: u64) -> batch::Rules {
        batch::Rules {
            window_secs: i64::try_from(window_secs).unwrap_or(i64::MAX),
            kept: self.versions_kept.value,
            history_max_bytes: self.history_max_bytes.value,
        }
    }

    /// The limits a document shows to its readers.
    #[must_use]
    pub fn shown(&self) -> DocumentLimits {
        DocumentLimits {
            bytes: self.max_bytes.value,
            blocks: self.max_blocks.value,
            file_bytes: self.file_max_bytes.value,
            files_bytes: self.files_max_bytes.value,
        }
    }

    /// Where the value of a limit is set; none for one the contract fixes.
    #[must_use]
    pub fn scope_of(&self, limit: Limit) -> Option<Id> {
        match limit {
            Limit::Bytes => self.max_bytes.scope,
            Limit::Blocks => self.max_blocks.scope,
            Limit::FileBytes => self.file_max_bytes.scope,
            Limit::FilesBytes => self.files_max_bytes.scope,
            Limit::BlockBytes | Limit::Depth | Limit::Operations => None,
        }
    }

    /// The value of a limit.
    #[must_use]
    pub fn value_of(&self, limit: Limit) -> i64 {
        match limit {
            Limit::Bytes => self.max_bytes.value,
            Limit::Blocks => self.max_blocks.value,
            Limit::FileBytes => self.file_max_bytes.value,
            Limit::FilesBytes => self.files_max_bytes.value,
            Limit::BlockBytes | Limit::Depth | Limit::Operations => {
                fixed(limit).unwrap_or_default()
            }
        }
    }
}

/// The node `id` as the caller may know it, for a refusal to name.
async fn node_named(
    conn: &mut Conn,
    principal: &Principal,
    id: Option<Id>,
    policy: Policy,
) -> Option<orch_api_contract::nodes::NodeRef> {
    let id = id?;
    view::refs(conn, principal, &[id], policy)
        .await
        .ok()
        .and_then(|found| found.get(&id).cloned())
}

/// `403 docs.disabled`, naming the node whose setting switched documents off.
pub async fn disabled(
    conn: &mut Conn,
    principal: &Principal,
    settings: &DocSettings,
    policy: Policy,
) -> ApiError {
    let scope = node_named(conn, principal, settings.enabled_scope, policy).await;
    ApiError::new(ErrorCode::DocsDisabled, "documents are switched off here").with("scope", scope)
}

/// `413 docs.too_large`: which limit, its value, and the node that sets it.
pub async fn too_large(
    conn: &mut Conn,
    principal: &Principal,
    settings: &DocSettings,
    limit: Limit,
    policy: Policy,
) -> ApiError {
    let value = settings.value_of(limit);
    let scope = node_named(conn, principal, settings.scope_of(limit), policy).await;
    ApiError::new(
        ErrorCode::DocsTooLarge,
        format!("a document may hold at most {value} {}", limit.what()),
    )
    .with("limit", limit.word())
    .with("value", value)
    .with("scope", scope)
}
