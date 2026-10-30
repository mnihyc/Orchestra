//! Exact native history ownership, measurements and removal in managed Codex homes.

use std::io::{self, BufRead, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::history::{NativeHistoryFact, NativePrefixSource};
use orch_types::Json;
use serde_json::json;

use crate::protocol::{MAX_FRAME_BYTES, METHOD_NOT_FOUND, Rpc, RpcError};

mod spool;

// Match the control-plane batch size while bounding each indexed native response independently.
const INVENTORY_PAGE: usize = 200;

// The header carries the same native input as a protocol frame; malformed history cannot allocate a whole rollout.
const MAX_METADATA_HEADER_BYTES: usize = MAX_FRAME_BYTES;

fn failure(code: &str) -> AdapterError {
    AdapterError::Other {
        code: code.into(),
        message: "The exact native history could not be managed safely.".into(),
        retryable: false,
    }
}

fn io_error(error: io::Error) -> AdapterError {
    AdapterError::HarnessError(format!("Native history storage: {error}"))
}

/// Rejects links and traversal before reading or removing a file beneath the managed home.
fn checked(home: &Path, path: &Path) -> Result<PathBuf, AdapterError> {
    if !home.is_absolute()
        || std::fs::symlink_metadata(home)
            .map_err(io_error)?
            .file_type()
            .is_symlink()
    {
        return Err(failure("history.unmanaged_home"));
    }
    let relative = path
        .strip_prefix(home)
        .map_err(|_| failure("history.outside_home"))?;
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(failure("history.outside_home"));
    }
    let mut current = home.to_path_buf();
    for part in relative.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(failure("history.symlink"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(path.to_path_buf())
}

/// Native metadata and lineage edges are indexed on disk once per home batch.
#[derive(Default)]
pub(crate) struct Inventory {
    storage: Mutex<Option<Index>>,
}

struct Index {
    threads: spool::Map<Json>,
    heads: spool::Map<u64>,
    edges: spool::Log<Edge>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Edge {
    native: String,
    next: Option<u64>,
}

impl Inventory {
    fn with_cancel(cancel: Arc<AtomicBool>) -> Result<Self, AdapterError> {
        Ok(Self {
            storage: Mutex::new(Some(Index {
                threads: spool::Map::with_cancel(Arc::clone(&cancel)).map_err(io_error)?,
                heads: spool::Map::with_cancel(Arc::clone(&cancel)).map_err(io_error)?,
                edges: spool::Log::new(cancel).map_err(io_error)?,
            })),
        })
    }

    /// Native opens keep the indexed snapshot aware of newly created forks.
    pub(crate) fn observe(&self, thread: &Json) -> Result<(), AdapterError> {
        let Some(native) = thread["id"].as_str() else {
            return Err(failure("history.inventory_unavailable"));
        };
        let mut storage = self.storage.lock().unwrap_or_else(PoisonError::into_inner);
        if storage.is_none() {
            *storage = Self::with_cancel(Arc::default())?
                .storage
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner);
        }
        let index = storage
            .as_mut()
            .ok_or_else(|| failure("history.inventory_unavailable"))?;
        if index.threads.get(native).map_err(io_error)?.is_none() {
            for (relation, parent) in [
                ("child", thread["parentThreadId"].as_str()),
                ("fork", thread["forkedFromId"].as_str()),
            ] {
                if let Some(parent) = parent {
                    let key = format!("{relation}:{parent}");
                    let next = index.heads.get(&key).map_err(io_error)?;
                    let offset = index
                        .edges
                        .append(&Edge {
                            native: native.to_owned(),
                            next,
                        })
                        .map_err(io_error)?;
                    index.heads.insert(&key, &offset).map_err(io_error)?;
                }
            }
        }
        index.threads.insert(native,&json!({"id":native,"path":thread["path"],"status":thread["status"],"parentThreadId":thread["parentThreadId"],"forkedFromId":thread["forkedFromId"]})).map_err(io_error)
    }

    fn thread(&self, native: &str) -> Result<Option<Json>, AdapterError> {
        let mut storage = self.storage.lock().unwrap_or_else(PoisonError::into_inner);
        match storage.as_mut() {
            Some(index) => index.threads.get(native).map_err(io_error),
            None => Ok(None),
        }
    }

    fn edges(&self, relation: &str, native: &str) -> Result<Edges<'_>, AdapterError> {
        let mut storage = self.storage.lock().unwrap_or_else(PoisonError::into_inner);
        let next = match storage.as_mut() {
            Some(index) => index
                .heads
                .get(&format!("{relation}:{native}"))
                .map_err(io_error)?,
            None => None,
        };
        Ok(Edges {
            inventory: self,
            next,
        })
    }
}

struct Edges<'a> {
    inventory: &'a Inventory,
    next: Option<u64>,
}
impl Iterator for Edges<'_> {
    type Item = Result<String, AdapterError>;
    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.next.take()?;
        Some((|| {
            let mut storage = self
                .inventory
                .storage
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let index = storage
                .as_mut()
                .ok_or_else(|| failure("history.inventory_unavailable"))?;
            let edge = index.edges.at(offset).map_err(io_error)?;
            self.next = edge.next;
            Ok(edge.native)
        })())
    }
}

/// Reads indexed native metadata in fixed pages and keeps no whole-home maps in memory.
pub(crate) async fn inventory(rpc: &Rpc) -> Result<Inventory, AdapterError> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut guard = spool::CancelOnDrop(Some(Arc::clone(&cancel)));
    let mut inventory = Inventory::with_cancel(Arc::clone(&cancel))?;
    for archived in [false, true] {
        let mut cursor: Option<String> = None;
        let mut cursors = spool::Map::<bool>::with_cancel(Arc::clone(&cancel)).map_err(io_error)?;
        loop {
            let page=rpc.call("thread/list",json!({"modelProviders":[],"sourceKinds":["cli","vscode","exec","appServer","subAgent","subAgentReview","subAgentCompact","subAgentThreadSpawn","subAgentOther","unknown"],"archived":archived,"useStateDbOnly":true,"cursor":cursor,"limit":INVENTORY_PAGE})).await.map_err(crate::map_rpc)?;
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            let next = cursor.clone();
            (inventory, cursors) = tokio::task::spawn_blocking(move || {
                let data = page["data"]
                    .as_array()
                    .filter(|data| data.len() <= INVENTORY_PAGE)
                    .ok_or_else(|| failure("history.inventory_unavailable"))?;
                for thread in data {
                    inventory.observe(thread)?;
                }
                if let Some(next) = next {
                    if cursors.get(&next).map_err(io_error)?.is_some() {
                        return Err(failure("history.inventory_unavailable"));
                    }
                    cursors.insert(&next, &true).map_err(io_error)?;
                }
                Ok::<_, AdapterError>((inventory, cursors))
            })
            .await
            .map_err(|error| AdapterError::Internal(error.to_string()))??;
            if cursor.is_none() {
                break;
            }
        }
    }
    guard.disarm();
    Ok(inventory)
}

fn files(home: &Path, path: &Path, native: &str) -> Result<Vec<PathBuf>, AdapterError> {
    let plain = if path.extension().is_some_and(|extension| extension == "zst") {
        path.with_extension("")
    } else {
        path.to_path_buf()
    };
    let relative = plain
        .strip_prefix(home)
        .map_err(|_| failure("history.outside_home"))?;
    if !matches!(relative.components().next(), Some(Component::Normal(root)) if root == "sessions" || root == "archived_sessions")
    {
        return Err(failure("history.outside_home"));
    }
    let name = plain
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| failure("history.identity_changed"))?;
    if !name.ends_with(&format!("-{native}.jsonl")) {
        return Err(failure("history.identity_changed"));
    }
    let active = if matches!(relative.components().next(), Some(Component::Normal(root)) if root == "sessions")
    {
        plain.clone()
    } else {
        // The native rollout filename starts with an ISO calendar date, not a retention limit.
        const ROLLOUT_DATE_CHARS: usize = 10;
        let date = name
            .strip_prefix("rollout-")
            .and_then(|stamp| stamp.get(..ROLLOUT_DATE_CHARS))
            .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            .ok_or_else(|| failure("history.path_unknown"))?;
        home.join("sessions")
            .join(date.format("%Y/%m/%d").to_string())
            .join(name)
    };
    let archived = home.join("archived_sessions").join(name);
    let mut paths = vec![
        plain.clone(),
        PathBuf::from(format!("{}.zst", plain.display())),
        active.clone(),
        PathBuf::from(format!("{}.zst", active.display())),
        archived.clone(),
        PathBuf::from(format!("{}.zst", archived.display())),
    ];
    paths.sort();
    paths.dedup();
    paths.into_iter().map(|path| checked(home, &path)).collect()
}

fn measure(paths: &[PathBuf], native: &str) -> Result<i64, AdapterError> {
    let mut bytes = 0_i64;
    for path in paths {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error(error)),
        };
        if !metadata.is_file() {
            return Err(failure("history.identity_changed"));
        }
        let file = std::fs::File::open(path).map_err(io_error)?;
        let reader: Box<dyn io::Read> =
            if path.extension().is_some_and(|extension| extension == "zst") {
                Box::new(zstd::stream::read::Decoder::new(file).map_err(io_error)?)
            } else {
                Box::new(file)
            };
        let mut first = String::new();
        let maximum =
            u64::try_from(MAX_METADATA_HEADER_BYTES).map_err(|_| failure("history.too_large"))?;
        let read = io::BufReader::new(reader.take(maximum.saturating_add(1)))
            .read_line(&mut first)
            .map_err(io_error)?;
        if read > MAX_METADATA_HEADER_BYTES {
            return Err(failure("history.too_large"));
        }
        let header: Json =
            serde_json::from_str(&first).map_err(|_| failure("history.identity_changed"))?;
        if header["type"] != "session_meta" || header["payload"]["id"] != native {
            return Err(failure("history.identity_changed"));
        }
        bytes = bytes
            .checked_add(i64::try_from(metadata.len()).map_err(|_| failure("history.too_large"))?)
            .ok_or_else(|| failure("history.too_large"))?;
    }
    Ok(bytes)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Member {
    native: String,
    paths: Vec<PathBuf>,
}
struct Family {
    members: spool::Log<Member>,
    seen: spool::Map<bool>,
    cursors: spool::Map<bool>,
    bytes: i64,
    cancel: Arc<AtomicBool>,
}

fn family(
    home: &Path,
    inventory: &Inventory,
    root: Json,
    native: &str,
    remove: bool,
    cancel: Arc<AtomicBool>,
) -> Result<Family, AdapterError> {
    let mut queue = spool::Log::new(Arc::clone(&cancel)).map_err(io_error)?;
    queue.append(&native.to_owned()).map_err(io_error)?;
    let mut family = Family {
        members: spool::Log::new(Arc::clone(&cancel)).map_err(io_error)?,
        seen: spool::Map::with_cancel(Arc::clone(&cancel)).map_err(io_error)?,
        cursors: spool::Map::with_cancel(Arc::clone(&cancel)).map_err(io_error)?,
        bytes: 0,
        cancel,
    };
    let mut cursor = 0;
    while let Some(id) = queue.next(&mut cursor).map_err(io_error)? {
        spool::check(&family.cancel).map_err(io_error)?;
        if family.seen.get(&id).map_err(io_error)?.is_some() {
            return Err(failure("history.invalid_lineage"));
        }
        family.seen.insert(&id, &true).map_err(io_error)?;
        let metadata = if id == native {
            root.clone()
        } else {
            inventory
                .thread(&id)?
                .ok_or_else(|| failure("history.inventory_unavailable"))?
        };
        if metadata["id"] != id {
            return Err(failure("history.identity_changed"));
        }
        if remove && metadata["status"]["type"] == "active" {
            return Err(failure("history.turn_running"));
        }
        let path = metadata["path"]
            .as_str()
            .ok_or_else(|| failure("history.path_unknown"))?;
        let paths = files(home, Path::new(path), &id)?;
        family.bytes = family
            .bytes
            .checked_add(measure(&paths, &id)?)
            .ok_or_else(|| failure("history.too_large"))?;
        family
            .members
            .append(&Member {
                native: id.clone(),
                paths,
            })
            .map_err(io_error)?;
        for child in inventory.edges("child", &id)? {
            queue.append(&child?).map_err(io_error)?;
        }
    }
    if remove {
        let mut cursor = 0;
        while let Some(member) = family.members.next(&mut cursor).map_err(io_error)? {
            for fork in inventory.edges("fork", &member.native)? {
                let fork = fork?;
                let thread = inventory
                    .thread(&fork)?
                    .ok_or_else(|| failure("history.inventory_unavailable"))?;
                let path = thread["path"]
                    .as_str()
                    .ok_or_else(|| failure("history.path_unknown"))?;
                if measure(&files(home, Path::new(path), &fork)?, &fork)? > 0
                    || thread["status"]["type"] == "active"
                {
                    return Err(failure("history.in_use"));
                }
            }
        }
    }
    Ok(family)
}

async fn unloaded(rpc: &Rpc, mut family: Family) -> Result<Family, AdapterError> {
    let mut cursor: Option<String> = None;
    loop {
        let loaded = rpc
            .call(
                "thread/loaded/list",
                json!({"cursor":cursor,"limit":INVENTORY_PAGE}),
            )
            .await
            .map_err(crate::map_rpc)?;
        cursor = loaded["nextCursor"].as_str().map(str::to_owned);
        let next = cursor.clone();
        family = tokio::task::spawn_blocking(move || {
            let data = loaded["data"]
                .as_array()
                .filter(|data| data.len() <= INVENTORY_PAGE)
                .ok_or_else(|| failure("history.idle_unconfirmed"))?;
            for id in data {
                let id = id.as_str().ok_or_else(|| failure("history.in_use"))?;
                if family.seen.get(id).map_err(io_error)?.is_some() {
                    return Err(failure("history.in_use"));
                }
            }
            if let Some(next) = next {
                if family.cursors.get(&next).map_err(io_error)?.is_some() {
                    return Err(failure("history.idle_unconfirmed"));
                }
                family.cursors.insert(&next, &true).map_err(io_error)?;
            }
            Ok(family)
        })
        .await
        .map_err(|error| AdapterError::Internal(error.to_string()))??;
        if cursor.is_none() {
            return Ok(family);
        }
    }
}

fn finish(
    home: &Path,
    mut family: Family,
    source: NativePrefixSource,
    remove: bool,
    fallback: bool,
) -> Result<NativeHistoryFact, AdapterError> {
    if remove {
        let mut cursor = 0;
        while let Some(member) = family.members.next(&mut cursor).map_err(io_error)? {
            for path in member.paths {
                spool::check(&family.cancel).map_err(io_error)?;
                if fallback {
                    checked(home, &path)?;
                    match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(io_error(error)),
                    }
                }
                if path.exists() {
                    return Err(failure("history.removal_unconfirmed"));
                }
            }
        }
    }
    Ok(NativeHistoryFact {
        source,
        bytes: Some(family.bytes),
        removed: remove,
        error: None,
    })
}

/// Uses native delete, or the exact verified and unloaded family of rollout files when the method is absent.
pub(crate) async fn manage(
    home: &Path,
    rpc: &Rpc,
    inventory: Arc<Inventory>,
    source: NativePrefixSource,
    remove: bool,
) -> Result<NativeHistoryFact, AdapterError> {
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = spool::CancelOnDrop(Some(Arc::clone(&cancel)));
    let root = match rpc
        .call(
            "thread/read",
            json!({"threadId":source.native_session_ref,"includeTurns":false}),
        )
        .await
    {
        Ok(answer) => answer["thread"].clone(),
        Err(error) => {
            let error = crate::map_rpc(error);
            if error.code() == "session_not_found" {
                let id = source.native_session_ref.clone();
                let absent = tokio::task::spawn_blocking(move || {
                    Ok::<_, AdapterError>(
                        inventory.thread(&id)?.is_none()
                            && inventory.edges("child", &id)?.next().transpose()?.is_none(),
                    )
                })
                .await
                .map_err(|error| AdapterError::Internal(error.to_string()))??;
                if absent {
                    return Ok(NativeHistoryFact {
                        source,
                        bytes: Some(0),
                        removed: true,
                        error: None,
                    });
                }
            }
            return Err(error);
        }
    };
    if root["id"] != source.native_session_ref {
        return Err(failure("history.identity_changed"));
    }
    let work_home = home.to_path_buf();
    let native = source.native_session_ref.clone();
    let family = tokio::task::spawn_blocking(move || {
        family(&work_home, &inventory, root, &native, remove, cancel)
    })
    .await
    .map_err(|error| AdapterError::Internal(error.to_string()))??;
    let (family, fallback) = if remove {
        match rpc
            .call(
                "thread/delete",
                json!({"threadId":source.native_session_ref}),
            )
            .await
        {
            Ok(_) => (family, false),
            Err(RpcError::Remote {
                code: METHOD_NOT_FOUND,
                ..
            }) => (unloaded(rpc, family).await?, true),
            Err(error) => return Err(crate::map_rpc(error)),
        }
    } else {
        (family, false)
    };
    let home = home.to_path_buf();
    tokio::task::spawn_blocking(move || finish(&home, family, source, remove, fallback))
        .await
        .map_err(|error| AdapterError::Internal(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_million_native_metadata_rows_use_paged_disk_indexes_instead_of_home_maps() {
        const CHILD: &str = "ORCH_NATIVE_METADATA_SCALE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output=std::process::Command::new("timeout").arg("10m").arg(std::env::current_exe().unwrap())
                .args(["--exact","native_history::tests::a_million_native_metadata_rows_use_paged_disk_indexes_instead_of_home_maps","--test-threads=1"])
                .env(CHILD,"1").output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        fn peak_bytes() -> u64 {
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .find_map(|line| {
                    line.strip_prefix("VmHWM:")
                        .and_then(|value| value.split_whitespace().next())
                        .and_then(|value| value.parse::<u64>().ok())
                })
                .unwrap()
                * 1024
        }
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        const ITEMS: usize = 1_000_000;
        // This accommodates bounded native frames, runtime stacks and allocator overhead, but no home map.
        const RETAINED_BUDGET: u64 = 32 * 1024 * 1024;
        const ROOT: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";
        let (adapter, peer) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(adapter);
        let (rpc, _incoming) = crate::protocol::connect(read, write, None);
        let (read, mut write) = tokio::io::split(peer);
        let server = tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            let mut requests = 0;
            while let Some(line) = lines.next_line().await.unwrap() {
                let asked: Json = serde_json::from_str(&line).unwrap();
                assert_eq!(asked["method"], "thread/list");
                assert_eq!(asked["params"]["limit"], INVENTORY_PAGE);
                let archived = asked["params"]["archived"].as_bool().unwrap();
                let start = asked["params"]["cursor"]
                    .as_str()
                    .map_or(0, |cursor| cursor.parse::<usize>().unwrap());
                let end = (start + INVENTORY_PAGE).min(ITEMS);
                let data: Vec<Json> = if archived {
                    Vec::new()
                } else {
                    (start..end).map(|item|{
                    let id=format!("00000000-0000-4000-8000-{item:012x}");
                    json!({"id":id,"path":format!("/workspace/sessions/rollout-{id}.jsonl"),"status":{"type":"idle"},"parentThreadId":ROOT,"forkedFromId":if item%10==0 {Some(ROOT)} else {None}})
                }).collect()
                };
                let next = (!archived && end < ITEMS).then(|| end.to_string());
                let response = json!({"id":asked["id"],"result":{"data":data,"nextCursor":next}});
                write
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
                requests += 1;
            }
            requests
        });
        let before = peak_bytes();
        let index = inventory(&rpc).await.unwrap();
        for item in [0, ITEMS / 2, ITEMS - 1] {
            let id = format!("00000000-0000-4000-8000-{item:012x}");
            assert_eq!(index.thread(&id).unwrap().unwrap()["id"], id);
        }
        assert_eq!(
            index
                .edges("child", ROOT)
                .unwrap()
                .map(Result::unwrap)
                .count(),
            ITEMS
        );
        assert_eq!(
            index
                .edges("fork", ROOT)
                .unwrap()
                .map(Result::unwrap)
                .count(),
            ITEMS / 10
        );
        assert!(
            peak_bytes().saturating_sub(before) < RETAINED_BUDGET,
            "whole-home native metadata must stay on disk"
        );
        rpc.finish();
        assert_eq!(server.await.unwrap(), ITEMS / INVENTORY_PAGE + 1);
    }

    #[tokio::test]
    async fn a_repeated_native_inventory_cursor_refuses_instead_of_scanning_forever() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (adapter, peer) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(adapter);
        let (rpc, _incoming) = crate::protocol::connect(read, write, None);
        let (read, mut write) = tokio::io::split(peer);
        let server = tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            let mut requests = 0;
            while let Some(line) = lines.next_line().await.unwrap() {
                let asked: Json = serde_json::from_str(&line).unwrap();
                assert_eq!(asked["method"], "thread/list");
                assert_eq!(asked["params"]["limit"], INVENTORY_PAGE);
                let response =
                    json!({"id":asked["id"],"result":{"data":[],"nextCursor":"repeated"}});
                write
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
                requests += 1;
            }
            requests
        });
        let result = inventory(&rpc).await;
        assert_eq!(
            result.err().unwrap().code(),
            "history.inventory_unavailable"
        );
        rpc.finish();
        assert_eq!(server.await.unwrap(), 2);
    }

    #[tokio::test]
    async fn an_independent_native_fork_refuses_removal_before_any_file_is_deleted() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let home = tempfile::tempdir().unwrap();
        let root = "11111111-1111-4111-8111-111111111111";
        let fork = "22222222-2222-4222-8222-222222222222";
        let directory = home.path().join("sessions/2026/10/27");
        std::fs::create_dir_all(&directory).unwrap();
        let root_path = directory.join(format!("rollout-2026-10-27T12-00-00-{root}.jsonl"));
        let fork_path = directory.join(format!("rollout-2026-10-27T12-00-00-{fork}.jsonl"));
        for (path, id) in [(&root_path, root), (&fork_path, fork)] {
            std::fs::write(
                path,
                format!("{}\n", json!({"type":"session_meta","payload":{"id":id}})),
            )
            .unwrap();
        }
        let index = Arc::new(Inventory::default());
        index
            .observe(
                &json!({"id":fork,"path":fork_path,"status":{"type":"idle"},"forkedFromId":root}),
            )
            .unwrap();
        let (adapter, peer) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(adapter);
        let (rpc, _incoming) = crate::protocol::connect(read, write, None);
        let (read, mut write) = tokio::io::split(peer);
        let path = root_path.clone();
        let server = tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            let mut requests = 0;
            while let Some(line) = lines.next_line().await.unwrap() {
                let asked: Json = serde_json::from_str(&line).unwrap();
                assert_eq!(
                    asked["method"], "thread/read",
                    "a needed fork must refuse before native delete"
                );
                let response = json!({"id":asked["id"],"result":{"thread":{"id":root,"path":path,"status":{"type":"idle"}}}});
                write
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
                requests += 1;
            }
            requests
        });
        let source = NativePrefixSource {
            thread_id: orch_types::new_id(),
            segment_id: orch_types::new_id(),
            native_session_ref: root.into(),
        };
        let error = manage(home.path(), &rpc, index, source, true)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "history.in_use");
        assert!(root_path.exists() && fork_path.exists());
        rpc.finish();
        assert_eq!(server.await.unwrap(), 1);
    }

    #[test]
    fn a_malformed_header_cannot_load_an_entire_large_rollout_or_authorize_its_removal() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join("oversized.jsonl");
        std::fs::File::create(&file)
            .unwrap()
            .set_len(u64::try_from(MAX_METADATA_HEADER_BYTES).unwrap() + 1)
            .unwrap();
        assert_eq!(
            measure(std::slice::from_ref(&file), "native")
                .unwrap_err()
                .code(),
            "history.too_large"
        );
        assert!(file.exists());
    }

    #[tokio::test]
    async fn a_missing_native_delete_frees_the_exact_idle_family_and_preserves_other_sessions() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let home = tempfile::tempdir().unwrap();
        let root = "11111111-1111-4111-8111-111111111111";
        let child = "22222222-2222-4222-8222-222222222222";
        let folder = home.path().join("sessions/2026/10/27");
        std::fs::create_dir_all(&folder).unwrap();
        let root_name = format!("rollout-2026-10-27T12-00-00-{root}.jsonl");
        let root_path = folder.join(&root_name);
        let archived = home.path().join("archived_sessions").join(&root_name);
        std::fs::create_dir_all(archived.parent().unwrap()).unwrap();
        let child_path = folder.join(format!("rollout-today-{child}.jsonl"));
        for (path, id) in [(&root_path, root), (&archived, root), (&child_path, child)] {
            std::fs::write(
                path,
                format!("{}\n", json!({"type":"session_meta","payload":{"id":id}})),
            )
            .unwrap();
        }
        let other = folder.join("other.jsonl");
        std::fs::write(&other, "keep this unrelated session").unwrap();
        let auth = home.path().join("auth.json");
        std::fs::write(&auth, "keep sign-in").unwrap();
        let source = NativePrefixSource {
            thread_id: orch_types::new_id(),
            segment_id: orch_types::new_id(),
            native_session_ref: root.into(),
        };
        let inventory = Inventory::default();
        inventory.observe(&json!({"id":child,"path":child_path,"status":{"type":"notLoaded"},"parentThreadId":root})).unwrap();
        let inventory = Arc::new(inventory);
        let (adapter, peer) = tokio::io::duplex(1 << 16);
        let (read, write) = tokio::io::split(adapter);
        let (rpc, _events) = crate::protocol::connect(read, write, None);
        let (read, mut write) = tokio::io::split(peer);
        let path = archived.clone();
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(read).lines();
            let mut methods = Vec::new();
            while let Some(line) = reader.next_line().await.unwrap() {
                let request: Json = serde_json::from_str(&line).unwrap();
                let method = request["method"].as_str().unwrap();
                methods.push(method.to_owned());
                let response = match method {
                    "thread/read" if !path.exists() => {
                        json!({"id":request["id"],"error":{"code":-32602,"message":"thread not found"}})
                    }
                    "thread/read" => {
                        json!({"id":request["id"],"result":{"thread":{"id":root,"path":path,"status":{"type":"idle"}}}})
                    }
                    "thread/delete" => {
                        json!({"id":request["id"],"error":{"code":METHOD_NOT_FOUND,"message":"method missing"}})
                    }
                    "thread/loaded/list" => {
                        json!({"id":request["id"],"result":{"data":[],"nextCursor":null}})
                    }
                    other => panic!("unexpected native request {other}"),
                };
                write
                    .write_all(format!("{response}\n").as_bytes())
                    .await
                    .unwrap();
                write.flush().await.unwrap();
            }
            methods
        });
        let result = manage(
            home.path(),
            &rpc,
            Arc::clone(&inventory),
            source.clone(),
            true,
        )
        .await
        .unwrap();
        assert!(result.removed && result.bytes.unwrap() > 0);
        assert!(!root_path.exists() && !child_path.exists());
        assert!(!archived.exists());
        assert_eq!(
            std::fs::read_to_string(other).unwrap(),
            "keep this unrelated session"
        );
        assert_eq!(std::fs::read_to_string(auth).unwrap(), "keep sign-in");
        // A missing parent does not prove its native descendants are absent too.
        std::fs::write(
            &child_path,
            format!(
                "{}\n",
                json!({"type":"session_meta","payload":{"id":child}})
            ),
        )
        .unwrap();
        assert_eq!(
            manage(home.path(), &rpc, Arc::clone(&inventory), source, false)
                .await
                .unwrap_err()
                .code(),
            "session_not_found"
        );
        assert!(child_path.exists());
        rpc.finish();
        assert_eq!(
            server.await.unwrap(),
            [
                "thread/read",
                "thread/delete",
                "thread/loaded/list",
                "thread/read"
            ]
        );
    }

    #[test]
    fn paths_cannot_escape_a_managed_home_or_follow_a_link() {
        let home = tempfile::tempdir().unwrap();
        assert!(checked(home.path(), &home.path().join("sessions/../auth.json")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(home.path(), home.path().join("sessions")).unwrap();
            assert_eq!(
                checked(home.path(), &home.path().join("sessions/anything"))
                    .unwrap_err()
                    .code(),
                "history.symlink"
            );
        }
    }
}
