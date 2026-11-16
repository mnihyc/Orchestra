//! One native directory level, pinned beneath the machine's current allowed root.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_api_contract::resources::MachineDirectoryPage;
use orch_envhost::{
    paths::RelPath,
    platform::{Dir, Kind, Meta},
};
use orch_link_contract::{
    commands::{CommandError, ErrorCode, MachineDirectoryList},
    events::PathStyle,
    policy::LocalPolicy,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

type Key = (usize, String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Version {
    device: u64,
    inode: u64,
    modified: i64,
    changed: i64,
}
impl From<Meta> for Version {
    fn from(meta: Meta) -> Self {
        Self {
            device: meta.dev,
            inode: meta.ino,
            modified: meta.mtime_ns,
            changed: meta.ctime_ns,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    binding: [u8; 32],
    version: Version,
    last: Key,
}

struct Admitted {
    root: PathBuf,
    path: PathBuf,
    directory: Dir,
    version: Version,
    binding: [u8; 32],
}

fn denied(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(
        ErrorCode::PolicyRootNotAllowed,
        format!("environment.roots: {why}"),
    )
}
fn changed() -> CommandError {
    CommandError::new(
        ErrorCode::Conflict,
        "The directory or its choices changed; read this level again",
    )
}
fn io(error: impl std::fmt::Display) -> CommandError {
    CommandError::new(
        ErrorCode::Internal,
        format!("The directory cannot be read: {error}"),
    )
}
fn budget() -> CommandError {
    CommandError::new(
        ErrorCode::TooLarge,
        "The directory page allowance is too small for this entry",
    )
}
fn check(cancel: &CancellationToken, due: Instant) -> Result<(), CommandError> {
    if cancel.is_cancelled() {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "The directory read was cancelled",
        ));
    }
    if Instant::now() >= due {
        return Err(CommandError::new(
            ErrorCode::Timeout,
            "Reading the directory exceeded its command deadline",
        )
        .retryable());
    }
    Ok(())
}

fn open(body: &MachineDirectoryList, policy: &LocalPolicy) -> Result<Admitted, CommandError> {
    if body.limit == 0 || body.page_bytes == 0 {
        return Err(budget());
    }
    if !policy
        .environment_roots
        .iter()
        .any(|root| Path::new(root) == Path::new(&body.root))
    {
        return Err(denied(
            "the selected root is no longer offered by this machine",
        ));
    }
    let style = if cfg!(windows) {
        PathStyle::Windows
    } else {
        PathStyle::Posix
    };
    let relative = RelPath::parse(&body.path, style).map_err(denied)?;
    if body.prefix.contains(['/', '\\', '\0']) {
        return Err(denied("complete one directory level at a time"));
    }
    let root = crate::policy::inspect_environment_directory(policy, Path::new(&body.root))?;
    let selected = crate::policy::inspect_environment_directory(
        policy,
        &Path::new(&body.root).join(relative.as_str()),
    )?;
    let root = PathBuf::from(root.path);
    let path = PathBuf::from(&selected.path);
    if !path.starts_with(&root) {
        return Err(denied("the directory is outside the selected root"));
    }
    let directory = Dir::open_path(&path).map_err(io)?;
    let meta = directory.stat_self().map_err(io)?;
    if selected.device.is_some_and(|device| device != meta.dev)
        || selected.inode.is_some_and(|inode| inode != meta.ino)
    {
        return Err(changed());
    }
    let encoded = serde_json::to_vec(&(
        &body.root,
        &body.path,
        &body.prefix,
        &body.recent,
        &policy.environment_roots,
        &root,
        &path,
    ))
    .map_err(io)?;
    let binding: [u8; 32] = Sha256::digest(encoded).into();
    Ok(Admitted {
        root,
        path,
        directory,
        version: meta.into(),
        binding,
    })
}

fn continuation(
    body: &MachineDirectoryList,
    admitted: &Admitted,
) -> Result<Option<Key>, CommandError> {
    let Some(cursor) = &body.cursor else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| changed())?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| changed())?;
    if cursor.binding != admitted.binding || cursor.version != admitted.version {
        return Err(changed());
    }
    Ok(Some(cursor.last))
}

fn child(
    admitted: &Admitted,
    policy: &LocalPolicy,
    name: &std::ffi::OsStr,
) -> Result<bool, CommandError> {
    let meta = match admitted.directory.lstat(name) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(changed()),
        Err(error) => return Err(io(error)),
    };
    match meta.kind {
        Kind::Dir => Ok(true),
        Kind::Symlink => Ok(crate::policy::inspect_environment_directory(
            policy,
            &admitted.path.join(name),
        )
        .is_ok_and(|target| Path::new(&target.path).starts_with(&admitted.root))),
        _ => Ok(false),
    }
}

fn weight(name: &str) -> Result<u64, CommandError> {
    Ok(u64::try_from(serde_json::to_vec(name).map_err(io)?.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1))
}

// Retain one ordered page and one lookahead, rather than a collected directory or subtree.
fn scan(
    body: &MachineDirectoryList,
    policy: &LocalPolicy,
    admitted: &Admitted,
    after: Option<&Key>,
    cancel: &CancellationToken,
    due: Instant,
) -> Result<(BTreeSet<Key>, bool), CommandError> {
    let recent: HashMap<_, _> = body
        .recent
        .iter()
        .enumerate()
        .map(|(at, name)| (name.as_str(), at))
        .collect();
    let prefix = body.prefix.to_lowercase();
    let count = usize::try_from(body.limit)
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    let mut selected = BTreeSet::new();
    let mut bytes = 0_u64;
    let mut more = false;
    for name in admitted.directory.names_stream().map_err(io)? {
        check(cancel, due)?;
        let name = name.map_err(io)?;
        let Some(text) = name.to_str() else {
            continue;
        };
        if !text.to_lowercase().starts_with(&prefix) || !child(admitted, policy, &name)? {
            continue;
        }
        let key = (
            recent.get(text).copied().unwrap_or(usize::MAX),
            text.to_owned(),
        );
        if after.is_some_and(|after| &key <= after) {
            continue;
        }
        let size = weight(text)?;
        if selected.insert(key) {
            bytes = bytes.saturating_add(size);
        }
        while selected.len() > count || (selected.len() > 1 && bytes > body.page_bytes) {
            let Some((_, name)) = selected.pop_last() else {
                break;
            };
            bytes = bytes.saturating_sub(weight(&name)?);
            more = true;
        }
    }
    if selected.len() > usize::try_from(body.limit).unwrap_or(usize::MAX) {
        let _ = selected.pop_last();
        more = true;
    }
    Ok((selected, more))
}

fn page(
    body: &MachineDirectoryList,
    admitted: &Admitted,
    mut selected: BTreeSet<Key>,
    mut more: bool,
    cancel: &CancellationToken,
    due: Instant,
) -> Result<MachineDirectoryPage, CommandError> {
    loop {
        check(cancel, due)?;
        if selected.is_empty() && more {
            return Err(budget());
        }
        let next_cursor = if more {
            selected
                .last()
                .cloned()
                .map(|last| {
                    serde_json::to_vec(&Cursor {
                        binding: admitted.binding,
                        version: admitted.version.clone(),
                        last,
                    })
                })
                .transpose()
                .map_err(io)?
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        } else {
            None
        };
        let result = MachineDirectoryPage {
            entries: selected.iter().map(|(_, name)| name.clone()).collect(),
            next_cursor,
        };
        if u64::try_from(serde_json::to_vec(&result).map_err(io)?.len()).unwrap_or(u64::MAX)
            <= body.page_bytes
        {
            return Ok(result);
        }
        if selected.pop_last().is_none() {
            return Err(budget());
        }
        more = true;
    }
}

/// Reads only metadata from one held directory and rechecks its native identity before returning it.
pub(crate) fn read(
    body: &MachineDirectoryList,
    policy: &LocalPolicy,
    cancel: &CancellationToken,
    due: Instant,
) -> Result<MachineDirectoryPage, CommandError> {
    check(cancel, due)?;
    let admitted = open(body, policy)?;
    let after = continuation(body, &admitted)?;
    let (selected, more) = scan(body, policy, &admitted, after.as_ref(), cancel, due)?;
    let current = open(body, policy)?;
    if current.version != admitted.version || current.binding != admitted.binding {
        return Err(changed());
    }
    page(body, &admitted, selected, more, cancel, due)
}
