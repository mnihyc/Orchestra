//! The skills a session is given, laid out for its thread.
//!
//! Each version of a skill arrives as an archive of its files (gzip tar, `SKILL.md` at the top), fetched by
//! `session.open` like its other files and checked against its hash. It is unpacked once into the connector's
//! cache, `<data>/skills/versions/<hash>/`, through a scratch directory renamed into place: a version there is whole
//! or absent and never changes. A thread's skills are `<data>/skills/threads/<thread>/`: `skills/<name>` is a link to
//! the version it is given, and nothing else is written there. Each program's adapter takes the same directory in
//! its own way (a capability root, a plugin it builds from the root, the root as it is). The thread's directory stays
//! where it is; its links are changed to the versions it is given when a session is opened, and a link no longer
//! given is removed. Nothing is written into a home another thread shares.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use orch_agent_contract::session::SessionSkills;
use orch_types::{Hash, Id};

/// The directory below the connector's data that holds the cache and the threads' directories.
const SKILLS_DIR: &str = "skills";
/// The most entries an archive of a skill may hold.
const MAX_ENTRIES: usize = 10_000;
/// The most bytes an archive of a skill may unpack to.
const MAX_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;

/// Why a thread's skills could not be laid out.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    /// The archive is not the shape of a skill.
    #[error("{0}")]
    Shape(String),
    /// Something on the disk failed.
    #[error("{what}: {source}")]
    Io {
        /// What was being done.
        what: String,
        /// What the system said.
        source: std::io::Error,
    },
}

fn io(what: impl Into<String>) -> impl FnOnce(std::io::Error) -> SkillError {
    let what = what.into();
    move |source| SkillError::Io { what, source }
}

/// Whether `name` can be one directory's name: letters, digits, `-`, `_` and `.`, not starting with a dot.
fn is_plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The connector's skills: the cache of versions and the threads' directories.
#[derive(Debug, Clone)]
pub struct SkillCache {
    root: PathBuf,
}

impl SkillCache {
    /// The skills of the connector whose data is in `data_dir`.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join(SKILLS_DIR),
        }
    }

    /// Where a version is, or would be.
    #[must_use]
    pub fn version_dir(&self, blob: &Hash) -> PathBuf {
        self.root.join("versions").join(blob.to_string())
    }

    /// The directory of the thread `thread`.
    #[must_use]
    pub fn thread_dir(&self, thread: Id) -> PathBuf {
        self.root.join("threads").join(thread.to_string())
    }

    /// Unpacks the archive at `archive` as the version `blob`, unless that version is there already; answers where
    /// it is.
    ///
    /// # Errors
    /// The archive is not a skill's (a path or a link leaving it, no `SKILL.md`, too large), or the disk failed.
    pub fn unpack(&self, blob: &Hash, archive: &Path) -> Result<PathBuf, SkillError> {
        let target = self.version_dir(blob);
        if target.join("SKILL.md").is_file() {
            return Ok(target);
        }
        let versions = self.root.join("versions");
        std::fs::create_dir_all(&versions).map_err(io("making the cache of skills"))?;
        let scratch = tempfile::Builder::new()
            .prefix(".unpacking-")
            .tempdir_in(&versions)
            .map_err(io("making a scratch directory for a skill"))?;
        let file = std::fs::File::open(archive).map_err(io("opening a skill's archive"))?;
        unpack_into(flate2::read::GzDecoder::new(file), scratch.path())?;
        if !scratch.path().join("SKILL.md").is_file() {
            return Err(SkillError::Shape(
                "a skill's archive has SKILL.md at its top".to_owned(),
            ));
        }
        let unpacked = scratch.keep();
        match std::fs::rename(&unpacked, &target) {
            Ok(()) => Ok(target),
            // Another open unpacked the same version first: theirs is the same files.
            Err(_) if target.join("SKILL.md").is_file() => {
                let _ = std::fs::remove_dir_all(&unpacked);
                Ok(target)
            }
            Err(source) => {
                let _ = std::fs::remove_dir_all(&unpacked);
                Err(SkillError::Io {
                    what: "putting a skill into the cache".to_owned(),
                    source,
                })
            }
        }
    }

    /// Lays out the directory of `thread` with exactly `skills` (each a name and the directory of its version), and
    /// answers it as a session is given it.
    ///
    /// # Errors
    /// A name is not one a directory can have, or the disk failed.
    pub fn lay_out(
        &self,
        thread: Id,
        skills: &[(String, PathBuf)],
    ) -> Result<SessionSkills, SkillError> {
        let dir = self.thread_dir(thread);
        let links = dir.join("skills");
        std::fs::create_dir_all(&links).map_err(io("making a thread's directory of skills"))?;
        let mut names = Vec::new();
        for (name, version) in skills {
            if !is_plain(name) {
                return Err(SkillError::Shape(format!(
                    "{name:?} is not a name a skill's directory can have"
                )));
            }
            let link = links.join(name);
            if std::fs::read_link(&link).ok().as_deref() != Some(version.as_path()) {
                // A new link is made beside the old one and renamed over it, so the name is never missing.
                let fresh = links.join(format!(".{name}.new"));
                remove_entry(&fresh);
                link_version(version, &fresh).map_err(io("linking a skill for a thread"))?;
                #[cfg(not(unix))]
                remove_entry(&link);
                std::fs::rename(&fresh, &link).map_err(io("linking a skill for a thread"))?;
            }
            names.push(name.clone());
        }
        for entry in std::fs::read_dir(&links)
            .map_err(io("reading a thread's skills"))?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !names.contains(&name) {
                remove_entry(&entry.path());
            }
        }
        Ok(SessionSkills {
            dir: dir.display().to_string(),
            names,
        })
    }
}

/// Points `link` at a version: a symbolic link on Unix. Windows lets only a privileged account or developer mode
/// make one, so there the link is a copy of the version's files; a version never changes, so the copy shows what the
/// link would, and it is made again whenever the thread is given another version.
fn link_version(version: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(version, link)
    }
    #[cfg(not(unix))]
    {
        copy_tree(version, link)
    }
}

/// Copies the directory `from` to `to`, files and directories only, as a version holds.
#[cfg(not(unix))]
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Removes a thread's entry for a skill, whether it is a link or (on Windows) a copy.
fn remove_entry(path: &Path) {
    if std::fs::remove_file(path).is_err() {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Unpacks a tar stream into `target`, refusing anything that is not a plain file or directory inside it.
fn unpack_into(reader: impl Read, target: &Path) -> Result<(), SkillError> {
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    let (mut entries, mut bytes) = (0_usize, 0_u64);
    for entry in archive.entries().map_err(io("reading a skill's archive"))? {
        let mut entry = entry.map_err(io("reading an entry of a skill's archive"))?;
        entries += 1;
        bytes = bytes.saturating_add(entry.size());
        if entries > MAX_ENTRIES || bytes > MAX_UNPACKED_BYTES {
            return Err(SkillError::Shape(
                "the archive holds more than a skill can".to_owned(),
            ));
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(SkillError::Shape(
                "a skill's archive holds only files and directories".to_owned(),
            ));
        }
        let path = entry
            .path()
            .map_err(io("reading a path of a skill's archive"))?
            .into_owned();
        if path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(SkillError::Shape(format!(
                "the archive has the path {} which leaves the skill's directory",
                path.display()
            )));
        }
        let executable = entry.header().mode().is_ok_and(|mode| mode & 0o111 != 0);
        if !entry.unpack_in(target).map_err(io("unpacking a skill"))? {
            return Err(SkillError::Shape(
                "the archive has an entry that leaves the skill's directory".to_owned(),
            ));
        }
        // A file's mode says whether a script runs, on systems that have modes.
        #[cfg(unix)]
        if kind.is_file() {
            use std::os::unix::fs::PermissionsExt;
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(target.join(&path), std::fs::Permissions::from_mode(mode))
                .map_err(io("setting a skill file's mode"))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(files: &[(&str, &str, u32)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        for (path, text, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            builder
                .append_data(&mut header, path, text.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn a_version_is_unpacked_once_and_a_thread_is_given_exactly_its_skills() {
        let data = tempfile::tempdir().unwrap();
        let cache = SkillCache::new(data.path());
        let file = data.path().join("one.tar.gz");
        std::fs::write(
            &file,
            archive(&[
                ("SKILL.md", "---\nname: greet\n---\nSay hi.\n", 0o644),
                ("scripts/run.sh", "echo hi\n", 0o755),
            ]),
        )
        .unwrap();
        let blob: Hash = "aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11"
            .parse()
            .unwrap();
        let version = cache.unpack(&blob, &file).unwrap();
        assert!(version.join("SKILL.md").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(version.join("scripts/run.sh"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        assert_eq!(
            cache.unpack(&blob, &file).unwrap(),
            version,
            "a version in the cache is taken as it is"
        );

        let thread = orch_types::new_id();
        let given = cache
            .lay_out(thread, &[("greet".to_owned(), version.clone())])
            .unwrap();
        let dir = PathBuf::from(&given.dir);
        assert_eq!(given.names, ["greet"]);
        assert!(dir.join("skills/greet/SKILL.md").is_file());
        assert!(
            !dir.join(".claude-plugin").exists(),
            "what a program loads the directory as is its adapter's to make; the connector writes only the links"
        );
        let emptied = cache.lay_out(thread, &[]).unwrap();
        assert_eq!(
            emptied.dir, given.dir,
            "the thread's directory stays where it is"
        );
        assert!(
            !dir.join("skills/greet").exists(),
            "a skill no longer given is taken out"
        );
    }

    #[test]
    fn an_archive_that_leaves_its_directory_or_holds_no_skill_is_refused() {
        let data = tempfile::tempdir().unwrap();
        let cache = SkillCache::new(data.path());
        let blob: Hash = "bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22"
            .parse()
            .unwrap();
        let file = data.path().join("bad.tar.gz");
        std::fs::write(&file, archive(&[("notes.md", "no skill here", 0o644)])).unwrap();
        assert!(matches!(
            cache.unpack(&blob, &file),
            Err(SkillError::Shape(_))
        ));
        assert!(
            !cache.version_dir(&blob).exists(),
            "nothing half-made is left in the cache"
        );
        assert!(
            cache
                .lay_out(
                    orch_types::new_id(),
                    &[("../out".to_owned(), data.path().to_path_buf())]
                )
                .is_err()
        );
    }
}
