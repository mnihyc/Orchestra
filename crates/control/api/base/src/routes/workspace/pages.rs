//! Directory continuation selectors and bounded metadata windows; none grants authority.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

use base64::Engine as _;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub use orch_api_contract::common::ErrorCode;

pub use orch_api_contract::workspace::DirEntry;

pub use orch_api_contract::workspace::DirEntryKind;

pub use orch_link_contract::commands::FsListPageResult;

pub use orch_types::Hash;

pub use orch_types::Id;

pub use orch_types::Time;

use serde::Deserialize;

use serde::Serialize;

pub use std::cmp::Ordering;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    Live,
    Manifest { blob: Hash },
    Opened,
}

#[derive(Debug, Clone, Serialize)]
pub struct Binding {
    pub environment: Id,
    pub generation: i32,
    pub machine: Option<Id>,
    pub path: String,
    pub mount: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub directory: bool,
    pub name: String,
}

impl Key {
    pub fn of(entry: &DirEntry) -> Self {
        Self {
            directory: entry.kind == DirEntryKind::Dir,
            name: entry.name.clone(),
        }
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        (!self.directory)
            .cmp(&(!other.directory))
            .then_with(|| self.name.to_lowercase().cmp(&other.name.to_lowercase()))
            .then_with(|| self.name.cmp(&other.name))
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Position {
    Native { cursor: String },
    Cached { after: Key },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub binding: Hash,
    pub revision: String,
    pub position: Position,
    pub as_of: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_from: Option<Time>,
    pub expires_at: Time,
}

pub struct Metadata {
    pub revision: String,
    pub next_cursor: Option<String>,
    pub as_of: Time,
    pub observed_from: Option<Time>,
    pub expires_at: Time,
}

orch_api_base::api_family_callback! {
    bind __api_bind_5406945ec5, __api_install_5406945ec5, __api_callback_5406945ec5;
    slot __ApiCallback5406945ec5, __API_CALLBACK_5406945EC5, __api_set_5406945ec5;
    base orch_api_base::routes::workspace::pages;

    fn stale() -> orch_api_base::middleware::requests::ApiError;
}

impl Binding {
    pub fn hash(&self) -> ApiResult<Hash> {
        serde_json::to_vec(self)
            .map(|bytes| Hash::of(&bytes))
            .map_err(ApiError::internal)
    }

    pub fn cursor(&self, text: Option<&str>, now: Time) -> ApiResult<Option<Cursor>> {
        text.map(|text| {
            if text.len() > orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES {
                return Err(stale());
            }
            let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| stale())?;
            let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| stale())?;
            if cursor.binding != self.hash()? || cursor.expires_at <= now {
                return Err(stale());
            }
            Ok(cursor)
        })
        .transpose()
    }

    pub fn agrees(&self, other: &Self) -> ApiResult<bool> {
        Ok(self.hash()? == other.hash()?)
    }

    pub fn metadata(
        &self,
        revision: String,
        position: Option<Position>,
        as_of: Time,
        now: Time,
    ) -> ApiResult<Metadata> {
        let expires_at = Time::new(now.get() + chrono::Duration::minutes(5));
        self.metadata_until(revision, position, as_of, None, expires_at)
    }

    pub fn metadata_until(
        &self,
        revision: String,
        position: Option<Position>,
        as_of: Time,
        observed_from: Option<Time>,
        expires_at: Time,
    ) -> ApiResult<Metadata> {
        let next_cursor = position
            .map(|position| {
                serde_json::to_vec(&Cursor {
                    binding: self.hash()?,
                    revision: revision.clone(),
                    position,
                    as_of,
                    observed_from,
                    expires_at,
                })
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
                .map_err(ApiError::internal)
            })
            .transpose()?;
        Ok(Metadata {
            revision,
            next_cursor,
            as_of,
            observed_from,
            expires_at,
        })
    }

    pub fn native(&self, page: FsListPageResult, now: Time) -> ApiResult<Metadata> {
        if page.observed_from.is_some_and(|start| start > page.as_of) {
            return Err(ApiError::new(
                ErrorCode::EnvironmentUnavailable,
                "The connector answered an invalid directory observation interval.",
            ));
        }
        let expires_at = page
            .expires_at
            .min(Time::new(now.get() + chrono::Duration::minutes(5)));
        self.metadata_until(
            format!("live:{}", page.revision),
            page.next_cursor.map(|cursor| Position::Native { cursor }),
            page.as_of,
            page.observed_from,
            expires_at,
        )
    }
}
