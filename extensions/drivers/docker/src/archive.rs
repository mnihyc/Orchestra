//! Tar archives in and out of the driver, and the blobs they come from.
//!
//! The daemon takes and gives files as tar streams (to put a file into a container, to copy one out, to load an
//! image, to build one). This module is the one place that writes and reads them: the files put into a unit, the
//! image archives read for an import, the build contexts assembled from an upload, and the one trait
//! ([`BlobSource`]) through which the driver is given the content of a blob: the node owns the means of fetching
//! one, the driver only asks for it by hash.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use async_trait::async_trait;
use futures::StreamExt;
use orch_blob_contract::store::{BlobError, BlobStore, ByteStream, Verifier};
use orch_driver_contract::driver::DriverError;
use orch_types::{Hash, Json};
use tokio::io::AsyncWriteExt;

/// The largest entry of an image archive that is read into memory: a manifest or a configuration, which are a few
/// kilobytes; a megabyte is more than any of them holds and keeps a hostile archive from filling memory.
const MAX_METADATA_BYTES: u64 = 1 << 20;

/// Where the driver gets the content of a blob: an image archive or a build context a template names by hash.
#[async_trait]
pub trait BlobSource: Send + Sync + 'static {
    /// The content of the blob `hash`, as a stream.
    async fn open(&self, hash: &Hash) -> Result<ByteStream, BlobError>;
}

/// Any blob store is a source of blobs.
#[async_trait]
impl<T: BlobStore> BlobSource for T {
    async fn open(&self, hash: &Hash) -> Result<ByteStream, BlobError> {
        Ok(self.get(hash, None).await?.body)
    }
}

/// A file of a tar archive that is written: its place, its content, the user and group it belongs to and its mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A directory.
    Dir {
        /// Its path in the archive, relative.
        path: String,
        /// Its permissions.
        mode: u32,
        /// The user it belongs to.
        uid: u32,
        /// The group it belongs to.
        gid: u32,
    },
    /// A regular file.
    File {
        /// Its path in the archive, relative.
        path: String,
        /// Its content.
        content: Vec<u8>,
        /// Its permissions.
        mode: u32,
        /// The user it belongs to.
        uid: u32,
        /// The group it belongs to.
        gid: u32,
    },
}

/// Writes a tar archive of `entries`, every one stamped with `mtime` (seconds since the epoch).
pub fn tar_of(entries: &[Entry], mtime: u64) -> Result<Vec<u8>, DriverError> {
    let internal =
        |error: std::io::Error| DriverError::Internal(format!("writing an archive: {error}"));
    let mut builder = tar::Builder::new(Vec::new());
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mtime(mtime);
        match entry {
            Entry::Dir {
                path,
                mode,
                uid,
                gid,
            } => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(*mode);
                header.set_uid(u64::from(*uid));
                header.set_gid(u64::from(*gid));
                // A directory's name ends in a slash, so that it is told from a file by name too.
                let name = format!("{}/", path.trim_end_matches('/'));
                builder
                    .append_data(&mut header, name, std::io::empty())
                    .map_err(internal)?;
            }
            Entry::File {
                path,
                content,
                mode,
                uid,
                gid,
            } => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(content.len() as u64);
                header.set_mode(*mode);
                header.set_uid(u64::from(*uid));
                header.set_gid(u64::from(*gid));
                builder
                    .append_data(&mut header, path, content.as_slice())
                    .map_err(internal)?;
            }
        }
    }
    builder.into_inner().map_err(internal)
}

/// The content of the first regular file of a tar archive, which is what the daemon gives for a path that is
/// a file.
#[must_use]
pub fn first_file(archive: &[u8]) -> Option<Vec<u8>> {
    let mut archive = tar::Archive::new(archive);
    for entry in archive.entries().ok()? {
        let mut entry = entry.ok()?;
        if entry.header().entry_type().is_file() {
            let mut content = Vec::new();
            entry.read_to_end(&mut content).ok()?;
            return Some(content);
        }
    }
    None
}

/// The content of the blob `hash`, kept in a temporary file for as long as the returned handle lives, after
/// checking that it is that content. Nothing of it is held in memory.
pub async fn spool(
    source: &dyn BlobSource,
    hash: &Hash,
) -> Result<tempfile::NamedTempFile, DriverError> {
    let unavailable = |error: BlobError| {
        DriverError::Image(format!("the blob {hash} could not be fetched: {error}"))
    };
    let io = |error: std::io::Error| {
        DriverError::Internal(format!(
            "the blob {hash} could not be kept on the disk: {error}"
        ))
    };
    let mut stream = source.open(hash).await.map_err(unavailable)?;
    let file = tempfile::NamedTempFile::new().map_err(io)?;
    let mut writer = tokio::fs::File::from_std(file.reopen().map_err(io)?);
    let mut verifier = Verifier::new(*hash, None);
    while let Some(piece) = stream.next().await {
        let piece = piece.map_err(unavailable)?;
        verifier.update(&piece);
        writer.write_all(&piece).await.map_err(io)?;
    }
    writer.flush().await.map_err(io)?;
    verifier.finish().map_err(unavailable)?;
    Ok(file)
}

/// One image of an image archive: the names it carries and its configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageInArchive {
    /// The names (`repository:tag`) the archive gives it; none when it is nameless.
    pub names: Vec<String>,
    /// Its configuration, the JSON the registry format calls the image configuration.
    pub config: Json,
}

/// Opens an archive, through gzip when it is compressed.
fn open_archive(path: &Path) -> std::io::Result<Box<dyn Read>> {
    let mut file = std::fs::File::open(path)?;
    let mut head = [0_u8; 2];
    let read = file.read(&mut head)?;
    file.seek(SeekFrom::Start(0))?;
    if read == 2 && head == [0x1f, 0x8b] {
        Ok(Box::new(flate2::read::GzDecoder::new(file)))
    } else {
        Ok(Box::new(file))
    }
}

/// Reads the entries of a tar that have one of the paths given, in one pass over it. A path is compared without a
/// leading `./`.
fn read_entries(reader: impl Read, wanted: &[String]) -> std::io::Result<HashMap<String, Vec<u8>>> {
    let mut found = HashMap::new();
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry
            .path()?
            .to_string_lossy()
            .trim_start_matches("./")
            .to_owned();
        if wanted.contains(&path) && entry.header().entry_type().is_file() {
            let mut content = Vec::new();
            entry
                .by_ref()
                .take(MAX_METADATA_BYTES)
                .read_to_end(&mut content)?;
            found.insert(path, content);
            if found.len() == wanted.len() {
                break;
            }
        }
    }
    Ok(found)
}

fn json_of(content: &[u8], what: &str) -> Result<Json, DriverError> {
    serde_json::from_slice(content).map_err(|error| {
        DriverError::InvalidSpec(format!(
            "the {what} of the archive cannot be read: {}",
            printable(&error.to_string())
        ))
    })
}

/// How many characters of the text of a failure on a file the person gave are repeated to them.
const PRINTABLE_CHARS: usize = 200;

/// A failure on a file somebody uploaded, fit to be shown: a reader that meets bytes that are not what it expects
/// quotes them, and control characters in a message are noise at best. Those become `?` and the text is cut.
#[must_use]
pub fn printable(text: &str) -> String {
    let mut shown: String = text
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .take(PRINTABLE_CHARS)
        .collect();
    if text.chars().count() > PRINTABLE_CHARS {
        shown.push_str("...");
    }
    shown
}

/// The path of a blob of an OCI layout from its digest, `sha256:<hex>`.
fn oci_blob_path(digest: &str) -> Option<String> {
    let (algorithm, hex) = digest.split_once(':')?;
    Some(format!("blobs/{algorithm}/{hex}"))
}

/// Reads the images an image archive holds: what `docker save` writes (a `manifest.json` that lists them) or an
/// OCI layout (an `index.json`). `architecture` is the host's, to choose among the platforms of a multi-platform
/// image.
pub fn read_image_archive(
    path: &Path,
    architecture: &str,
) -> Result<Vec<ImageInArchive>, DriverError> {
    let io = |error: std::io::Error| {
        DriverError::InvalidSpec(format!(
            "the archive cannot be read: {}",
            printable(&error.to_string())
        ))
    };
    let first = read_entries(
        open_archive(path).map_err(io)?,
        &["manifest.json".to_owned(), "index.json".to_owned()],
    )
    .map_err(io)?;
    if let Some(manifest) = first.get("manifest.json") {
        let list = json_of(manifest, "manifest")?;
        let entries = list.as_array().cloned().unwrap_or_default();
        let wanted: Vec<String> = entries
            .iter()
            .filter_map(|entry| entry.get("Config").and_then(Json::as_str))
            .map(str::to_owned)
            .collect();
        let configs = read_entries(open_archive(path).map_err(io)?, &wanted).map_err(io)?;
        let mut images = Vec::new();
        for entry in &entries {
            let Some(config_path) = entry.get("Config").and_then(Json::as_str) else {
                continue;
            };
            let Some(config) = configs.get(config_path.trim_start_matches("./")) else {
                return Err(DriverError::InvalidSpec(format!(
                    "the archive has no {config_path}"
                )));
            };
            let names = entry
                .get("RepoTags")
                .and_then(Json::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(Json::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            images.push(ImageInArchive {
                names,
                config: json_of(config, "configuration")?,
            });
        }
        return Ok(images);
    }
    let Some(index) = first.get("index.json") else {
        return Err(DriverError::InvalidSpec(
            "the archive is no image archive: it has neither a manifest.json nor an index.json"
                .to_owned(),
        ));
    };
    let index = json_of(index, "index")?;
    let name_of = |descriptor: &Json| -> Vec<String> {
        let annotation = |key: &str| {
            descriptor
                .get("annotations")
                .and_then(|annotations| annotations.get(key))
                .and_then(Json::as_str)
                .map(str::to_owned)
        };
        annotation("io.containerd.image.name")
            .or_else(|| annotation("org.opencontainers.image.ref.name"))
            .into_iter()
            .collect()
    };
    // The descriptors of the index lead to manifests; a manifest that is itself an index of platforms leads on
    // to the one for this host. Two rounds reach any image layout in use.
    let mut pending: Vec<(Vec<String>, String)> = index
        .get("manifests")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|descriptor| {
            let digest = descriptor.get("digest").and_then(Json::as_str)?;
            Some((name_of(descriptor), digest.to_owned()))
        })
        .collect();
    let mut resolved: Vec<(Vec<String>, String)> = Vec::new();
    for _ in 0..2 {
        if pending.is_empty() {
            break;
        }
        let paths: Vec<String> = pending
            .iter()
            .filter_map(|(_, digest)| oci_blob_path(digest))
            .collect();
        let blobs = read_entries(open_archive(path).map_err(io)?, &paths).map_err(io)?;
        let mut next = Vec::new();
        for (names, digest) in pending {
            let Some(body) = oci_blob_path(&digest).and_then(|blob| blobs.get(&blob)) else {
                continue;
            };
            let manifest = json_of(body, "manifest")?;
            if let Some(platforms) = manifest.get("manifests").and_then(Json::as_array) {
                let for_host = platforms.iter().find(|platform| {
                    platform
                        .pointer("/platform/architecture")
                        .and_then(Json::as_str)
                        .is_some_and(|candidate| {
                            crate::imports::architecture_matches(candidate, architecture)
                        })
                });
                if let Some(chosen) = for_host
                    .or_else(|| platforms.first())
                    .and_then(|p| p.get("digest"))
                    .and_then(Json::as_str)
                {
                    next.push((names, chosen.to_owned()));
                }
            } else if let Some(config) = manifest.pointer("/config/digest").and_then(Json::as_str) {
                resolved.push((names, config.to_owned()));
            }
        }
        pending = next;
    }
    let config_paths: Vec<String> = resolved
        .iter()
        .filter_map(|(_, digest)| oci_blob_path(digest))
        .collect();
    let configs = read_entries(open_archive(path).map_err(io)?, &config_paths).map_err(io)?;
    let mut images = Vec::new();
    for (names, digest) in resolved {
        let Some(config) = oci_blob_path(&digest).and_then(|blob| configs.get(&blob)) else {
            continue;
        };
        images.push(ImageInArchive {
            names,
            config: json_of(config, "configuration")?,
        });
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_quoting_the_bytes_of_an_upload_is_shown_without_them() {
        let quoted = format!(
            "numeric field was not a number: {} when getting cksum",
            "\u{7}".repeat(300)
        );
        let shown = printable(&quoted);
        assert!(!shown.chars().any(char::is_control));
        assert!(shown.starts_with("numeric field was not a number: ???"));
        assert!(shown.ends_with("..."));
        assert!(shown.chars().count() <= PRINTABLE_CHARS + 3);
        assert_eq!(printable("an ordinary message"), "an ordinary message");
    }

    #[test]
    fn files_and_directories_are_written_with_their_user_group_and_mode_and_read_back() {
        let archive = tar_of(
            &[
                Entry::Dir {
                    path: "workspace".into(),
                    mode: 0o750,
                    uid: 4242,
                    gid: 4243,
                },
                Entry::File {
                    path: "run/orch/enroll".into(),
                    content: b"token".to_vec(),
                    mode: 0o600,
                    uid: 4242,
                    gid: 4243,
                },
            ],
            1_700_000_000,
        )
        .unwrap();
        let mut read = tar::Archive::new(archive.as_slice());
        let entries: Vec<_> = read.entries().unwrap().map(Result::unwrap).collect();
        assert_eq!(entries.len(), 2);
        let dir = entries[0].header();
        assert_eq!(entries[0].path().unwrap().to_str(), Some("workspace/"));
        assert!(dir.entry_type().is_dir());
        assert_eq!(
            (dir.mode().unwrap(), dir.uid().unwrap(), dir.gid().unwrap()),
            (0o750, 4242, 4243)
        );
        assert_eq!(dir.mtime().unwrap(), 1_700_000_000);
        assert_eq!(first_file(&archive), Some(b"token".to_vec()));
    }

    #[test]
    fn an_archive_without_a_file_has_no_first_file() {
        let archive = tar_of(
            &[Entry::Dir {
                path: "d".into(),
                mode: 0o755,
                uid: 0,
                gid: 0,
            }],
            0,
        )
        .unwrap();
        assert_eq!(first_file(&archive), None);
        assert_eq!(first_file(b"not an archive"), None);
    }
}
