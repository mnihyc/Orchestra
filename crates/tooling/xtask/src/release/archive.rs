//! Archives written the same way every time, and read back to see what is in them.
//!
//! A release is checked by its hash, so the same inputs have to make the same
//! bytes: entries are sorted, times are fixed, and no owner is recorded.

use crate::release::ReleaseError;
use flate2::{Compression, GzBuilder, read::GzDecoder};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The mode of a program or a script inside an archive.
pub const EXECUTABLE_MODE: u32 = 0o755;
/// The mode of every other file inside an archive.
pub const FILE_MODE: u32 = 0o644;

/// Where an archive entry's bytes come from.
#[derive(Debug, Clone)]
pub enum Source {
    /// A file on disk.
    File(PathBuf),
    /// Bytes made here.
    Bytes(Vec<u8>),
}

/// One file of an archive.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Its path inside the archive, with `/` between the parts.
    pub path: String,
    /// Its content.
    pub source: Source,
    /// Its mode.
    pub mode: u32,
}

impl Entry {
    /// A file on disk, kept at `path` in the archive with the mode it has.
    pub fn file(path: impl Into<String>, source: &Path) -> Result<Self, ReleaseError> {
        Ok(Self {
            path: path.into(),
            mode: mode_of(source)?,
            source: Source::File(source.to_owned()),
        })
    }

    /// Bytes made here, kept at `path`.
    #[must_use]
    pub fn bytes(path: impl Into<String>, bytes: Vec<u8>, mode: u32) -> Self {
        Self {
            path: path.into(),
            source: Source::Bytes(bytes),
            mode,
        }
    }

    fn content(&self) -> Result<Vec<u8>, ReleaseError> {
        match &self.source {
            Source::Bytes(bytes) => Ok(bytes.clone()),
            Source::File(path) => fs::read(path).map_err(|source| ReleaseError::Io {
                path: path.clone(),
                source,
            }),
        }
    }
}

#[cfg(unix)]
fn mode_of(path: &Path) -> Result<u32, ReleaseError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path).map_err(|source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    })?;
    let executable = metadata.permissions().mode() & 0o111 != 0;
    Ok(if executable {
        EXECUTABLE_MODE
    } else {
        FILE_MODE
    })
}

#[cfg(not(unix))]
fn mode_of(_path: &Path) -> Result<u32, ReleaseError> {
    Ok(FILE_MODE)
}

/// Every file under `dir` as entries below `prefix`, sorted by path, skipping
/// what `skip` refuses (it is given the path relative to `dir`, with `/`).
/// A link is refused: an archive cannot carry one the same way everywhere.
pub fn walk(
    dir: &Path,
    prefix: &str,
    skip: &dyn Fn(&str) -> bool,
) -> Result<Vec<Entry>, ReleaseError> {
    fn visit(
        base: &Path,
        dir: &Path,
        prefix: &str,
        skip: &dyn Fn(&str) -> bool,
        into: &mut Vec<Entry>,
    ) -> Result<(), ReleaseError> {
        let io = |source| ReleaseError::Io {
            path: dir.to_owned(),
            source,
        };
        for entry in fs::read_dir(dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            let relative = path
                .strip_prefix(base)
                .map_err(|_| {
                    ReleaseError::Invalid(format!("{} left {}", path.display(), base.display()))
                })?
                .to_string_lossy()
                .replace('\\', "/");
            if skip(&relative) {
                continue;
            }
            let kind = entry.file_type().map_err(io)?;
            if kind.is_symlink() {
                return Err(ReleaseError::Invalid(format!(
                    "{} is a link, which a release archive does not carry",
                    path.display()
                )));
            }
            if kind.is_dir() {
                visit(base, &path, prefix, skip, into)?;
            } else {
                into.push(Entry::file(format!("{prefix}{relative}"), &path)?);
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    visit(dir, dir, prefix, skip, &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Writes `entries` as a zip archive at `out`.
pub fn write_zip(out: &Path, entries: &[Entry]) -> Result<(), ReleaseError> {
    let io = |source| ReleaseError::Io {
        path: out.to_owned(),
        source,
    };
    let invalid = |error: zip::result::ZipError| {
        ReleaseError::Invalid(format!("cannot write {}: {error}", out.display()))
    };
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut writer = zip::ZipWriter::new(fs::File::create(out).map_err(io)?);
    for entry in sorted {
        // The time of every entry is the zip format's own earliest, so that
        // the archive does not depend on when it was made.
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(entry.mode)
            .last_modified_time(zip::DateTime::default());
        writer
            .start_file(entry.path.as_str(), options)
            .map_err(invalid)?;
        writer.write_all(&entry.content()?).map_err(io)?;
    }
    writer.finish().map_err(invalid)?;
    Ok(())
}

/// Writes `entries` as a gzip-compressed tar archive at `out`.
pub fn write_tar_gz(out: &Path, entries: &[Entry]) -> Result<(), ReleaseError> {
    let io = |source| ReleaseError::Io {
        path: out.to_owned(),
        source,
    };
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    // No modification time in the compressed stream either.
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(fs::File::create(out).map_err(io)?, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for entry in sorted {
        let content = entry.content()?;
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(entry.mode);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        builder
            .append_data(&mut header, entry.path.as_str(), content.as_slice())
            .map_err(io)?;
    }
    builder.into_inner().map_err(io)?.finish().map_err(io)?;
    Ok(())
}

/// The names of the files in the zip archive at `path`.
pub fn list_zip(path: &Path) -> Result<Vec<String>, ReleaseError> {
    let io = |source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    };
    let mut archive = zip::ZipArchive::new(fs::File::open(path).map_err(io)?).map_err(|error| {
        ReleaseError::Invalid(format!("{} is not a zip archive: {error}", path.display()))
    })?;
    let mut names = Vec::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| {
            ReleaseError::Invalid(format!("{} cannot be read: {error}", path.display()))
        })?;
        let name = file.name().map_err(|error| {
            ReleaseError::Invalid(format!("{} has a bad name: {error}", path.display()))
        })?;
        names.push(name.into_owned());
    }
    Ok(names)
}

/// The content of the file called `name` in the zip archive at `path`, if the
/// archive has one.
pub fn read_zip_entry(path: &Path, name: &str) -> Result<Option<Vec<u8>>, ReleaseError> {
    use std::io::Read;
    let io = |source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    };
    let mut archive = zip::ZipArchive::new(fs::File::open(path).map_err(io)?).map_err(|error| {
        ReleaseError::Invalid(format!("{} is not a zip archive: {error}", path.display()))
    })?;
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => {
            return Err(ReleaseError::Invalid(format!(
                "{} cannot be read: {error}",
                path.display()
            )));
        }
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io)?;
    Ok(Some(bytes))
}

/// The names of the files in the tar.gz archive at `path`, and the content of
/// the one called `read`, if there is one.
pub fn list_tar_gz(
    path: &Path,
    read: Option<&str>,
) -> Result<(Vec<String>, Option<Vec<u8>>), ReleaseError> {
    use std::io::Read;
    let io = |source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    };
    let mut archive = tar::Archive::new(GzDecoder::new(fs::File::open(path).map_err(io)?));
    let mut names = Vec::new();
    let mut content = None;
    for entry in archive.entries().map_err(io)? {
        let mut entry = entry.map_err(io)?;
        let name = entry.path().map_err(io)?.to_string_lossy().into_owned();
        if read == Some(name.as_str()) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(io)?;
            content = Some(bytes);
        }
        names.push(name);
    }
    Ok((names, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        vec![
            Entry::bytes("b/second", b"two".to_vec(), FILE_MODE),
            Entry::bytes("a/first", b"one".to_vec(), EXECUTABLE_MODE),
        ]
    }

    #[test]
    fn the_same_files_make_the_same_archive_whenever_it_is_made() {
        let dir = tempfile::tempdir().expect("a directory");
        for name in ["one.zip", "two.zip"] {
            write_zip(&dir.path().join(name), &entries()).expect("a zip");
        }
        for name in ["one.tgz", "two.tgz"] {
            write_tar_gz(&dir.path().join(name), &entries()).expect("a tar");
        }
        let read = |name: &str| fs::read(dir.path().join(name)).expect("read");
        assert_eq!(read("one.zip"), read("two.zip"));
        assert_eq!(read("one.tgz"), read("two.tgz"));
    }

    #[test]
    fn what_was_put_in_an_archive_is_listed_in_order() {
        let dir = tempfile::tempdir().expect("a directory");
        write_zip(&dir.path().join("a.zip"), &entries()).expect("a zip");
        assert_eq!(
            list_zip(&dir.path().join("a.zip")).expect("listed"),
            ["a/first", "b/second"]
        );
        write_tar_gz(&dir.path().join("a.tgz"), &entries()).expect("a tar");
        let (names, content) =
            list_tar_gz(&dir.path().join("a.tgz"), Some("b/second")).expect("listed");
        assert_eq!(names, ["a/first", "b/second"]);
        assert_eq!(content.as_deref(), Some(b"two".as_slice()));
    }

    #[test]
    fn walking_a_directory_skips_what_is_refused_and_keeps_the_prefix() {
        let dir = tempfile::tempdir().expect("a directory");
        fs::create_dir_all(dir.path().join("runtime/data")).expect("dirs");
        fs::write(dir.path().join("runtime/data/secret"), b"x").expect("file");
        fs::write(dir.path().join("compose.yml"), b"y").expect("file");
        let found =
            walk(dir.path(), "deploy/", &|path| path.starts_with("runtime")).expect("walked");
        let names: Vec<&str> = found.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(names, ["deploy/compose.yml"]);
    }
}
