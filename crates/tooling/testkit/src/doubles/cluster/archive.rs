//! The existing owned-volume archive format for the cluster model's regular files.

use super::{ClusterArchiveCodec, ClusterFile};
use orch_driver_contract::driver::DriverError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::{Component, Path};

/// The node helper's existing version marker and metadata bound, not a new transfer limit.
const MAGIC: &[u8] = b"ORCH-VOLUME-1\n";
const HEADER_MAX: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: Vec<u8>,
    kind: String,
    size: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    atime: i64,
    atime_ns: i64,
    mtime: i64,
    mtime_ns: i64,
    link: Option<Vec<u8>>,
    major: u64,
    minor: u64,
    xattrs: Vec<(Vec<u8>, Vec<u8>)>,
}

fn invalid(error: impl std::fmt::Display) -> DriverError {
    DriverError::InvalidSpec(error.to_string())
}
fn checked(path: &str) -> Result<(), DriverError> {
    if path.as_bytes().contains(&0)
        || !path.is_empty()
            && (path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
                || Path::new(path)
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_))))
    {
        return Err(invalid("a carried path must remain below its owned root"));
    }
    Ok(())
}
fn header(path: &str, kind: &str, file: &ClusterFile) -> Result<Entry, DriverError> {
    checked(path)?;
    Ok(Entry {
        path: path.as_bytes().to_vec(),
        kind: kind.into(),
        size: if kind == "file" {
            u64::try_from(file.data.len()).map_err(invalid)?
        } else {
            0
        },
        uid: u32::try_from(file.uid).map_err(invalid)?,
        gid: u32::try_from(file.gid).map_err(invalid)?,
        mode: if kind == "file" { file.mode } else { 0o755 },
        atime: file.atime,
        atime_ns: i64::from(file.atime_ns),
        mtime: file.mtime,
        mtime_ns: i64::from(file.mtime_ns),
        link: None,
        major: 0,
        minor: 0,
        xattrs: if kind == "file" {
            file.xattrs
                .iter()
                .map(|(key, value)| (key.as_bytes().to_vec(), value.clone()))
                .collect()
        } else {
            Vec::new()
        },
    })
}
fn append(output: &mut Vec<u8>, entry: &Entry) -> Result<(), DriverError> {
    let encoded = serde_json::to_vec(entry).map_err(invalid)?;
    if encoded.len() > HEADER_MAX {
        return Err(invalid(
            "the volume entry's metadata exceeds the archive bound",
        ));
    }
    output.extend_from_slice(&u32::try_from(encoded.len()).map_err(invalid)?.to_le_bytes());
    output.extend_from_slice(&encoded);
    Ok(())
}

/// A portable regular-file codec interoperable with the trusted node helper.
pub struct VolumeCodec;
impl ClusterArchiveCodec for VolumeCodec {
    fn encode(&self, files: &BTreeMap<String, ClusterFile>) -> Result<Vec<u8>, DriverError> {
        let mut output = MAGIC.to_vec();
        let root = files
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| ClusterFile {
                data: Vec::new(),
                uid: 0,
                gid: 0,
                mode: 0o755,
                atime: 0,
                atime_ns: 0,
                mtime: 0,
                mtime_ns: 0,
                xattrs: Default::default(),
            });
        append(&mut output, &header("", "directory", &root)?)?;
        let mut directories = BTreeSet::new();
        for path in files.keys() {
            checked(path)?;
            if path.is_empty() {
                return Err(invalid("a regular file cannot be the archive root"));
            }
            let mut parent = Path::new(path).parent();
            while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
                directories.insert(path.to_string_lossy().into_owned());
                parent = path.parent();
            }
        }
        for directory in &directories {
            if files.contains_key(directory) {
                return Err(invalid("a file cannot also be an archive directory parent"));
            }
            append(&mut output, &header(directory, "directory", &root)?)?;
        }
        for (path, file) in files {
            append(&mut output, &header(path, "file", file)?)?;
            output.extend_from_slice(&file.data);
        }
        output.extend_from_slice(&0u32.to_le_bytes());
        Ok(output)
    }
    fn decode(&self, data: &[u8]) -> Result<BTreeMap<String, ClusterFile>, DriverError> {
        let mut input = Cursor::new(data);
        let mut magic = vec![0; MAGIC.len()];
        input.read_exact(&mut magic).map_err(invalid)?;
        if magic != MAGIC {
            return Err(invalid("the volume archive version is not supported"));
        }
        let mut seen = BTreeMap::<String, String>::new();
        let mut files = BTreeMap::new();
        loop {
            let mut length = [0; 4];
            input.read_exact(&mut length).map_err(invalid)?;
            let length = usize::try_from(u32::from_le_bytes(length)).map_err(invalid)?;
            if length == 0 {
                break;
            }
            if length > HEADER_MAX {
                return Err(invalid("the carried metadata exceeds the archive bound"));
            }
            let mut bytes = vec![0; length];
            input.read_exact(&mut bytes).map_err(invalid)?;
            let entry: Entry = serde_json::from_slice(&bytes).map_err(invalid)?;
            let path = String::from_utf8(entry.path).map_err(invalid)?;
            checked(&path)?;
            if seen.is_empty() && (!path.is_empty() || entry.kind != "directory") {
                return Err(invalid("a volume archive begins with its root directory"));
            }
            if seen.contains_key(&path) {
                return Err(invalid("a volume archive repeats a path"));
            }
            if !path.is_empty() {
                let parent = Path::new(&path)
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    .to_string_lossy();
                if seen
                    .get(parent.as_ref())
                    .is_none_or(|kind| kind != "directory")
                {
                    return Err(invalid("a carried path needs an earlier directory parent"));
                }
            }
            if entry.kind == "file" {
                let remaining = u64::try_from(data.len())
                    .map_err(invalid)?
                    .saturating_sub(input.position());
                if entry.size > remaining
                    || !(0..1_000_000_000).contains(&entry.atime_ns)
                    || !(0..1_000_000_000).contains(&entry.mtime_ns)
                {
                    return Err(invalid("the file size or nanosecond time is invalid"));
                }
                let mut data = vec![0; usize::try_from(entry.size).map_err(invalid)?];
                input.read_exact(&mut data).map_err(invalid)?;
                let xattrs = entry
                    .xattrs
                    .into_iter()
                    .map(|(key, value)| {
                        String::from_utf8(key)
                            .map(|key| (key, value))
                            .map_err(invalid)
                    })
                    .collect::<Result<_, _>>()?;
                files.insert(
                    path.clone(),
                    ClusterFile {
                        data,
                        uid: u64::from(entry.uid),
                        gid: u64::from(entry.gid),
                        mode: entry.mode,
                        atime: entry.atime,
                        atime_ns: u32::try_from(entry.atime_ns).map_err(invalid)?,
                        mtime: entry.mtime,
                        mtime_ns: u32::try_from(entry.mtime_ns).map_err(invalid)?,
                        xattrs,
                    },
                );
            } else if entry.kind != "directory" || entry.size != 0 {
                return Err(DriverError::Unsupported(
                    "the cluster model stores regular files and their directory parents only"
                        .into(),
                ));
            }
            seen.insert(path, entry.kind);
        }
        if seen.is_empty() || input.position() != u64::try_from(data.len()).map_err(invalid)? {
            return Err(invalid(
                "the volume archive has no root or has trailing bytes",
            ));
        }
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_or_extra_archive_bytes_never_form_a_complete_owned_volume()
    -> Result<(), DriverError> {
        let archive = VolumeCodec.encode(&BTreeMap::new())?;
        assert!(VolumeCodec.decode(&archive)?.is_empty());
        assert!(VolumeCodec.decode(&archive[..archive.len() - 1]).is_err());
        let mut extra = archive.clone();
        extra.push(1);
        assert!(VolumeCodec.decode(&extra).is_err());
        let mut wrong_version = archive;
        wrong_version[0] = b'X';
        assert!(VolumeCodec.decode(&wrong_version).is_err());
        Ok(())
    }
}
