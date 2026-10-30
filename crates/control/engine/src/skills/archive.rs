//! A skill's files: read from an archive (a zip, or a tar with or without gzip), checked, listed in a manifest, and written as the one archive that each version keeps.
//!
//! Whatever a version comes from (an upload, a repository fetched on a machine, a directory, a draft),
//! its files end up here and leave as the same thing: a gzip-compressed tar of regular files, in the
//! order of their paths, each with the mode 0644 or, where it is executable, 0755, no owner and no
//! time. The same tree therefore always makes the same archive, and the store keeps it once.
//!
//! Nothing in an archive is followed or run. A name that climbs out of the skill, or begins at a
//! root, refuses the archive; so does a link, which would point at something outside what is kept;
//! the folders an archiving program adds of its own (`__MACOSX`, `.DS_Store`) and a repository's
//! `.git` are left out. Reading stops at the skill's size limit, however much an entry claims, so that
//! a small archive that unpacks into a great deal is refused before it is held.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Component, Path};

use orch_api_contract::skills::SkillFile;
use sha2::{Digest, Sha256};

use super::front_matter::{self, FrontMatterFault};

/// The file that makes a directory a skill, at its top.
pub const SKILL_FILE: &str = "SKILL.md";

/// The mode of a file that is executable, and of one that is not, as the archive writes them.
const MODE_EXECUTABLE: u32 = 0o755;
const MODE_FILE: u32 = 0o644;

/// The bits of a mode that make a file executable by anybody.
const EXECUTE_BITS: u32 = 0o111;

/// The first bytes of a zip, of a gzip stream, and the signature a tar header holds at its offset.
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
const ZIP_EMPTY_MAGIC: &[u8] = b"PK\x05\x06";
const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];
const TAR_MAGIC: &[u8] = b"ustar";
const TAR_MAGIC_OFFSET: usize = 257;

/// What one version may hold, as the place it is made in says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes its files hold together (`skills.max_bytes`).
    pub max_bytes: u64,
    /// The most files it holds (`skills.max_files`): the manifest of every version lists each one.
    pub max_files: usize,
}

/// What an archive adds to each file at most, as a bound: a tar's header and padding (1,024 bytes)
/// and a long name's own header and blocks, or a zip's two records with the name twice, for paths of
/// up to about fifteen hundred bytes.
const ENTRY_OVERHEAD_BYTES: u64 = 4096;

impl Limits {
    /// The most bytes an archive of a skill within these limits has: its files, and what the archive
    /// adds to each of as many files as a skill may hold. A larger archive is refused before it is
    /// read.
    #[must_use]
    pub fn archive_bytes(self) -> u64 {
        self.max_bytes.saturating_add(
            u64::try_from(self.max_files)
                .unwrap_or(u64::MAX)
                .saturating_mul(ENTRY_OVERHEAD_BYTES),
        )
    }
}

/// One file of a skill, as it is held while a version is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Where it lies in the skill: a relative path, `/` between its parts.
    pub path: String,
    /// What it holds.
    pub data: Vec<u8>,
    /// Whether it is executable.
    pub executable: bool,
}

/// A version, made: what its `SKILL.md` says, the manifest of its files, and its archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    /// The name its `SKILL.md` gives.
    pub name: String,
    /// The description its `SKILL.md` gives.
    pub description: String,
    /// Its files in the order of their paths.
    pub manifest: Vec<SkillFile>,
    /// The bytes all its files hold together.
    pub size: i64,
    /// The archive to keep: a gzip-compressed tar of the files.
    pub archive: Vec<u8>,
}

/// Why a skill's files were refused, in the words a person reads. The words name no file: the
/// paths, the folder and what a library said are given by [`SkillFault::about`], since a reply carries
/// what was written beside its words, not in them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillFault {
    /// The bytes are no zip and no tar.
    #[error(
        "This is not an archive Orch reads: a skill comes as a zip or a tar archive, compressed or not"
    )]
    NotAnArchive,
    /// The archive is damaged or uses what cannot be read.
    #[error("The archive cannot be read")]
    Unreadable(String),
    /// A name climbs out of the skill or begins at a root.
    #[error("A file lies outside the skill")]
    Outside(String),
    /// A link, which is not kept.
    #[error("A file is a link; a skill holds files only")]
    Link(String),
    /// Something that is neither a file nor a folder.
    #[error("An entry is neither a file nor a folder")]
    NotAFile(String),
    /// A name that is not text.
    #[error("A name in the archive is not text")]
    NameNotText,
    /// A path that is given twice.
    #[error("A file is in the archive twice")]
    Twice(String),
    /// An encrypted entry.
    #[error("A file is encrypted, and Orch opens no encrypted file")]
    Encrypted(String),
    /// The files are larger than the place allows.
    #[error("The skill's files hold more than the {limit} bytes allowed here")]
    TooLarge {
        /// The bound.
        limit: u64,
    },
    /// More files than a skill holds.
    #[error("The skill holds more than the {limit} files allowed here")]
    TooManyFiles {
        /// The bound.
        limit: usize,
    },
    /// The folder asked for is not in the archive.
    #[error("The folder named is not a folder of the archive")]
    NoFolder(String),
    /// No `SKILL.md` where a skill's is looked for.
    #[error("There is no skill file at the top of the archive, nor in its one folder")]
    NoSkillFile,
    /// `SKILL.md` is not text.
    #[error("The skill file is not UTF-8 text")]
    SkillFileNotText,
    /// The front matter of `SKILL.md` is refused.
    #[error("{0}")]
    FrontMatter(#[from] FrontMatterFault),
    /// A path given for one file of a draft is not one a skill may hold.
    #[error("A path is not one a skill's file may have")]
    BadPath(String),
}

impl SkillFault {
    /// What the refusal is about, as it was written: a file's path, the folder named, the skill file,
    /// or what reading the archive met.
    #[must_use]
    pub fn about(&self) -> Vec<String> {
        match self {
            Self::Unreadable(what)
            | Self::Outside(what)
            | Self::Link(what)
            | Self::NotAFile(what)
            | Self::Twice(what)
            | Self::Encrypted(what)
            | Self::NoFolder(what)
            | Self::BadPath(what) => vec![what.clone()],
            Self::NoSkillFile | Self::SkillFileNotText => vec![SKILL_FILE.to_owned()],
            Self::FrontMatter(fault) => {
                let mut about = vec![SKILL_FILE.to_owned()];
                about.extend(fault.about());
                about
            }
            Self::NotAnArchive
            | Self::NameNotText
            | Self::TooLarge { .. }
            | Self::TooManyFiles { .. } => Vec::new(),
        }
    }
}

/// The files of an archive, the skill's own: from the folder `folder` when one is named, otherwise
/// from the top of the archive or, where everything lies in one folder, from that folder.
pub fn read_archive(
    bytes: &[u8],
    folder: Option<&str>,
    limits: Limits,
) -> Result<Vec<File>, SkillFault> {
    let files = if bytes.starts_with(ZIP_MAGIC) || bytes.starts_with(ZIP_EMPTY_MAGIC) {
        read_zip(bytes, limits)?
    } else if bytes.starts_with(GZIP_MAGIC) {
        read_tar(
            flate2::read::MultiGzDecoder::new(bytes),
            limits,
            Some(limits.archive_bytes()),
        )?
    } else if bytes
        .get(TAR_MAGIC_OFFSET..TAR_MAGIC_OFFSET + TAR_MAGIC.len())
        .is_some_and(|magic| magic == TAR_MAGIC)
    {
        read_tar(bytes, limits, Some(limits.archive_bytes()))?
    } else {
        return Err(SkillFault::NotAnArchive);
    };
    take_root(files, folder)
}

/// The files of a version's own archive, as [`build`] wrote it.
///
/// Its headers were generated here, rather than supplied by an importer. A later lower place
/// limit must not invalidate that frozen metadata; the stored file counts and content sizes
/// still bound its files. Source archives use the expanded-input guard in [`read_archive`].
pub fn unpack(archive: &[u8], limits: Limits) -> Result<Vec<File>, SkillFault> {
    read_tar(flate2::read::MultiGzDecoder::new(archive), limits, None)
}

/// A version made of `files`: its `SKILL.md` read and checked, every path checked, the manifest and
/// the archive written.
pub fn build(mut files: Vec<File>, limits: Limits) -> Result<Built, SkillFault> {
    if files.len() > limits.max_files {
        return Err(SkillFault::TooManyFiles {
            limit: limits.max_files,
        });
    }
    let mut size: u64 = 0;
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for file in &files {
        let normal =
            normal_path(&file.path).ok_or_else(|| SkillFault::BadPath(file.path.clone()))?;
        if normal != file.path {
            return Err(SkillFault::BadPath(file.path.clone()));
        }
        if seen.insert(file.path.clone(), ()).is_some() {
            return Err(SkillFault::Twice(file.path.clone()));
        }
        size = size.saturating_add(file.data.len() as u64);
    }
    if size > limits.max_bytes {
        return Err(SkillFault::TooLarge {
            limit: limits.max_bytes,
        });
    }
    let skill_file = files
        .iter()
        .find(|file| file.path == SKILL_FILE)
        .ok_or(SkillFault::NoSkillFile)?;
    let text = std::str::from_utf8(&skill_file.data).map_err(|_| SkillFault::SkillFileNotText)?;
    let front = front_matter::read(text)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = files
        .iter()
        .map(|file| SkillFile {
            path: file.path.clone(),
            size: i64::try_from(file.data.len()).unwrap_or(i64::MAX),
            sha256: hex::encode(Sha256::digest(&file.data)),
            executable: file.executable,
        })
        .collect();
    Ok(Built {
        name: front.name,
        description: front.description,
        manifest,
        size: i64::try_from(size).unwrap_or(i64::MAX),
        archive: write_archive(&files)?,
    })
}

/// The gzip-compressed tar of `files`, which are in the order of their paths.
fn write_archive(files: &[File]) -> Result<Vec<u8>, SkillFault> {
    let failed = |error: std::io::Error| SkillFault::Unreadable(error.to_string());
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for file in files {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(file.data.len() as u64);
        header.set_mode(if file.executable {
            MODE_EXECUTABLE
        } else {
            MODE_FILE
        });
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        builder
            .append_data(&mut header, &file.path, file.data.as_slice())
            .map_err(failed)?;
    }
    builder
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(failed)
}

/// `path` as a skill keeps it: its parts joined by `/`, without `.` parts; nothing where it is empty,
/// climbs out, begins at a root, or holds a part that cannot be a file's name.
#[must_use]
pub fn normal_path(path: &str) -> Option<String> {
    if path.contains('\0') || path.contains('\\') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Whether a file at `path` is one an archiving program or a repository adds of its own, which a
/// skill does not keep.
#[must_use]
pub fn left_out(path: &str) -> bool {
    path.split('/')
        .any(|part| part == ".git" || part == "__MACOSX")
        || path.rsplit('/').next() == Some(".DS_Store")
}

/// Counts what is read against the limits.
struct Budget {
    limits: Limits,
    bytes: u64,
    files: usize,
}

impl Budget {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            bytes: 0,
            files: 0,
        }
    }

    /// Reads one entry whole, refusing it as soon as the total passes the limit.
    fn read(&mut self, reader: impl Read) -> Result<Vec<u8>, SkillFault> {
        self.files += 1;
        if self.files > self.limits.max_files {
            return Err(SkillFault::TooManyFiles {
                limit: self.limits.max_files,
            });
        }
        let room = self.limits.max_bytes.saturating_sub(self.bytes);
        let mut data = Vec::new();
        reader
            .take(room.saturating_add(1))
            .read_to_end(&mut data)
            .map_err(|error| SkillFault::Unreadable(error.to_string()))?;
        self.bytes = self.bytes.saturating_add(data.len() as u64);
        if self.bytes > self.limits.max_bytes {
            return Err(SkillFault::TooLarge {
                limit: self.limits.max_bytes,
            });
        }
        Ok(data)
    }
}

fn read_zip(bytes: &[u8], limits: Limits) -> Result<Vec<File>, SkillFault> {
    let unreadable = |error: zip::result::ZipError| SkillFault::Unreadable(error.to_string());
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(unreadable)?;
    let mut budget = Budget::new(limits);
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(unreadable)?;
        let raw = String::from_utf8_lossy(entry.name_raw()).into_owned();
        let Some(enclosed) = entry.enclosed_name() else {
            return Err(SkillFault::Outside(raw));
        };
        let path = enclosed
            .to_str()
            .and_then(normal_path)
            .ok_or(SkillFault::NameNotText)?;
        if left_out(&path) || entry.is_dir() {
            continue;
        }
        if entry.is_symlink() {
            return Err(SkillFault::Link(path));
        }
        if entry.encrypted() {
            return Err(SkillFault::Encrypted(path));
        }
        let executable = entry
            .unix_mode()
            .is_some_and(|mode| mode & EXECUTE_BITS != 0);
        let data = budget.read(entry)?;
        files.push(File {
            path,
            data,
            executable,
        });
    }
    Ok(files)
}

/// An imported tar's entire expanded input has a budget, including metadata consumed by the archive library
/// before it returns an entry. Exhaustion is an error: an ordinary EOF could make a truncated stream
/// look complete after the skill file was already read.
struct ArchiveInput<R> {
    reader: R,
    remaining: Option<u64>,
}

impl<R: Read> Read for ArchiveInput<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let Some(remaining) = self.remaining else {
            return self.reader.read(buffer);
        };
        let room = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        if room == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                "the expanded archive exceeds its allowance",
            ));
        }
        let read = self.reader.read(&mut buffer[..room])?;
        self.remaining = Some(remaining - read as u64);
        Ok(read)
    }
}

fn read_tar(
    reader: impl Read,
    limits: Limits,
    input_limit: Option<u64>,
) -> Result<Vec<File>, SkillFault> {
    let unreadable = |error: std::io::Error| {
        if error.kind() == std::io::ErrorKind::FileTooLarge {
            SkillFault::TooLarge {
                limit: limits.max_bytes,
            }
        } else {
            SkillFault::Unreadable(error.to_string())
        }
    };
    let mut archive = tar::Archive::new(ArchiveInput {
        reader,
        remaining: input_limit,
    });
    let mut budget = Budget::new(limits);
    let mut files = Vec::new();
    for entry in archive.entries().map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        // Check before skipping a directory or repository file: reaching the next header would
        // otherwise decompress its whole body without going through the kept-file budget.
        if input_limit
            .is_some_and(|limit| entry.raw_file_position().saturating_add(entry.size()) > limit)
        {
            return Err(SkillFault::TooLarge {
                limit: limits.max_bytes,
            });
        }
        let raw = entry.path().map_err(unreadable)?;
        let shown = raw.to_string_lossy().into_owned();
        let text = raw.to_str().ok_or(SkillFault::NameNotText)?;
        let Some(path) = normal_path(text) else {
            if raw.components().all(|part| part == Component::CurDir) {
                continue;
            }
            return Err(SkillFault::Outside(shown));
        };
        let kind = entry.header().entry_type();
        if left_out(&path) || kind.is_dir() {
            continue;
        }
        if kind.is_symlink() || kind.is_hard_link() {
            return Err(SkillFault::Link(path));
        }
        if !(kind.is_file() || kind == tar::EntryType::Continuous) {
            return Err(SkillFault::NotAFile(path));
        }
        let executable = entry
            .header()
            .mode()
            .is_ok_and(|mode| mode & EXECUTE_BITS != 0);
        let data = budget.read(entry)?;
        files.push(File {
            path,
            data,
            executable,
        });
    }
    Ok(files)
}

/// The skill's files of an archive's files: those under `folder` when one is named, otherwise those
/// at the top where `SKILL.md` is there, or those of the archive's one folder where its `SKILL.md`
/// is, each path made relative to that place.
fn take_root(files: Vec<File>, folder: Option<&str>) -> Result<Vec<File>, SkillFault> {
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for file in &files {
        if seen.insert(file.path.clone(), ()).is_some() {
            return Err(SkillFault::Twice(file.path.clone()));
        }
    }
    let prefix = match folder.map(str::trim).filter(|folder| !folder.is_empty()) {
        Some(folder) => {
            let normal =
                normal_path(folder).ok_or_else(|| SkillFault::NoFolder(folder.to_owned()))?;
            let prefix = format!("{normal}/");
            if !files.iter().any(|file| file.path.starts_with(&prefix)) {
                return Err(SkillFault::NoFolder(normal));
            }
            prefix
        }
        None if files.iter().any(|file| file.path == SKILL_FILE) => String::new(),
        None => {
            let mut tops = files
                .iter()
                .map(|file| file.path.split('/').next().unwrap_or_default());
            let first = tops.next().ok_or(SkillFault::NoSkillFile)?;
            if !tops.all(|top| top == first) {
                return Err(SkillFault::NoSkillFile);
            }
            format!("{first}/")
        }
    };
    let taken: Vec<File> = files
        .into_iter()
        .filter_map(|file| {
            let path = file.path.strip_prefix(&prefix)?.to_owned();
            Some(File { path, ..file })
        })
        .collect();
    if !taken.iter().any(|file| file.path == SKILL_FILE) {
        return Err(SkillFault::NoSkillFile);
    }
    Ok(taken)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const LIMITS: Limits = Limits {
        max_bytes: 1024 * 1024,
        max_files: 100,
    };

    fn skill_md(name: &str) -> Vec<u8> {
        format!("---\nname: {name}\ndescription: Does a thing. Use it when asked.\n---\n# {name}\n")
            .into_bytes()
    }

    fn tar_of(entries: &[(&str, &[u8], u32)], gzip: bool) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, data, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            builder.append_data(&mut header, path, *data).unwrap();
        }
        let bytes = builder.into_inner().unwrap();
        if !gzip {
            return bytes;
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn zip_of(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (path, data, mode) in entries {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(*mode);
            writer.start_file(*path, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn a_tar_gz_with_one_folder_becomes_a_version_with_its_manifest() {
        let skill = skill_md("pdf-tools");
        let bytes = tar_of(
            &[
                ("pdf-tools/SKILL.md", &skill, 0o644),
                ("pdf-tools/scripts/run.sh", b"#!/bin/sh\necho hi\n", 0o755),
                ("pdf-tools/references/REFERENCE.md", b"# ref\n", 0o644),
            ],
            true,
        );
        let files = read_archive(&bytes, None, LIMITS).unwrap();
        let built = build(files, LIMITS).unwrap();
        assert_eq!(built.name, "pdf-tools");
        assert_eq!(built.description, "Does a thing. Use it when asked.");
        let paths: Vec<&str> = built
            .manifest
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(
            paths,
            ["SKILL.md", "references/REFERENCE.md", "scripts/run.sh"]
        );
        let script = &built.manifest[2];
        assert!(script.executable);
        assert_eq!(script.size, 18);
        assert_eq!(
            script.sha256,
            hex::encode(Sha256::digest(b"#!/bin/sh\necho hi\n"))
        );
        assert!(!built.manifest[0].executable);
        assert_eq!(
            built.size,
            built.manifest.iter().map(|file| file.size).sum::<i64>()
        );
    }

    #[test]
    fn the_same_files_always_make_the_same_archive_and_it_reads_back() {
        let skill = skill_md("same");
        let one = build(
            read_archive(
                &zip_of(&[("SKILL.md", &skill, 0o644), ("b.txt", b"b", 0o600)]),
                None,
                LIMITS,
            )
            .unwrap(),
            LIMITS,
        )
        .unwrap();
        let two = build(
            read_archive(
                &tar_of(
                    &[("b.txt", b"b", 0o640), ("./SKILL.md", &skill, 0o600)],
                    false,
                ),
                None,
                LIMITS,
            )
            .unwrap(),
            LIMITS,
        )
        .unwrap();
        assert_eq!(one.archive, two.archive);
        let back = unpack(&one.archive, LIMITS).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].path, "SKILL.md");
        assert_eq!(back[0].data, skill);
        assert_eq!(build(back, LIMITS).unwrap().archive, one.archive);
    }

    #[test]
    fn a_folder_inside_the_archive_can_be_chosen() {
        let bytes = tar_of(
            &[
                ("repo/README.md", b"# repo", 0o644),
                ("repo/skills/pdf/SKILL.md", &skill_md("pdf"), 0o644),
                ("repo/skills/xlsx/SKILL.md", &skill_md("xlsx"), 0o644),
            ],
            true,
        );
        assert_eq!(
            read_archive(&bytes, None, LIMITS),
            Err(SkillFault::NoSkillFile)
        );
        let files = read_archive(&bytes, Some("repo/skills/xlsx/"), LIMITS).unwrap();
        assert_eq!(build(files, LIMITS).unwrap().name, "xlsx");
        assert_eq!(
            read_archive(&bytes, Some("repo/none"), LIMITS),
            Err(SkillFault::NoFolder("repo/none".to_owned()))
        );
    }

    #[test]
    fn what_an_archiving_program_adds_of_its_own_is_left_out() {
        let bytes = zip_of(&[
            ("thing/SKILL.md", &skill_md("thing"), 0o644),
            ("__MACOSX/thing/._SKILL.md", b"junk", 0o644),
            ("thing/.DS_Store", b"junk", 0o644),
            ("thing/.git/HEAD", b"ref", 0o644),
        ]);
        let files = read_archive(&bytes, None, LIMITS).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "SKILL.md");
    }

    #[test]
    fn a_path_that_climbs_out_and_a_link_are_refused() {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        // The tar crate refuses to write `..` through its checked setter, as an attacker's would not.
        header.as_gnu_mut().unwrap().name[..9].copy_from_slice(b"../escape");
        header.set_cksum();
        builder.append(&header, &b"x"[..]).unwrap();
        let bytes = builder.into_inner().unwrap();
        assert!(matches!(
            read_archive(&bytes, None, LIMITS),
            Err(SkillFault::Outside(_) | SkillFault::Unreadable(_))
        ));

        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder
            .append_link(&mut header, "SKILL.md", "/etc/passwd")
            .unwrap();
        let bytes = builder.into_inner().unwrap();
        assert_eq!(
            read_archive(&bytes, None, LIMITS),
            Err(SkillFault::Link("SKILL.md".to_owned()))
        );
    }

    #[test]
    fn an_archive_that_unpacks_past_the_limit_is_refused() {
        let big = vec![b'a'; 4096];
        let bytes = tar_of(
            &[
                ("SKILL.md", &skill_md("big"), 0o644),
                ("big.txt", &big, 0o644),
            ],
            true,
        );
        let small = Limits {
            max_bytes: 1024,
            ..LIMITS
        };
        assert_eq!(
            read_archive(&bytes, None, small),
            Err(SkillFault::TooLarge { limit: 1024 })
        );
        let few = Limits {
            max_files: 1,
            ..LIMITS
        };
        assert_eq!(
            read_archive(&bytes, None, few),
            Err(SkillFault::TooManyFiles { limit: 1 })
        );
        let files = read_archive(&bytes, None, LIMITS).unwrap();
        assert_eq!(
            build(files, few).unwrap_err(),
            SkillFault::TooManyFiles { limit: 1 }
        );
    }

    #[test]
    fn a_broken_skill_is_refused_with_the_reason() {
        let no_skill = tar_of(&[("README.md", b"# no", 0o644)], false);
        assert_eq!(
            read_archive(&no_skill, None, LIMITS),
            Err(SkillFault::NoSkillFile)
        );
        let bad_name = tar_of(
            &[(
                "SKILL.md",
                b"---\nname: Bad_Name\ndescription: d\n---\n",
                0o644,
            )],
            false,
        );
        let files = read_archive(&bad_name, None, LIMITS).unwrap();
        let fault = build(files, LIMITS).unwrap_err();
        assert!(matches!(
            fault,
            SkillFault::FrontMatter(FrontMatterFault::Name { .. })
        ));
        assert_eq!(fault.about(), ["SKILL.md", "Bad_Name"]);
        assert_eq!(
            read_archive(b"just some text, not an archive", None, LIMITS),
            Err(SkillFault::NotAnArchive)
        );
        let binary = tar_of(&[("SKILL.md", &[0xff, 0xfe, 0x00], 0o644)], false);
        assert_eq!(
            build(read_archive(&binary, None, LIMITS).unwrap(), LIMITS),
            Err(SkillFault::SkillFileNotText)
        );
    }

    #[test]
    fn paths_of_a_draft_are_checked_before_they_are_kept() {
        let skill = skill_md("draft");
        let files = vec![
            File {
                path: "SKILL.md".to_owned(),
                data: skill.clone(),
                executable: false,
            },
            File {
                path: "a/../../b".to_owned(),
                data: Vec::new(),
                executable: false,
            },
        ];
        assert_eq!(
            build(files, LIMITS),
            Err(SkillFault::BadPath("a/../../b".to_owned()))
        );
        assert_eq!(normal_path("./a//b/./c"), Some("a/b/c".to_owned()));
        assert_eq!(normal_path("/etc/passwd"), None);
        assert_eq!(normal_path("a\\b"), None);
        assert_eq!(normal_path(""), None);
    }
}
