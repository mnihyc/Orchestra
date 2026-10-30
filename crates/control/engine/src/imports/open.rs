//! Opening what a person uploaded as the source of an import: an Orch package, with its manifest read and checked, or plain files, with the folders and documents they make.
//!
//! Nothing here writes anything. A package's manifest is read whole (it is bounded), checked for what it says of itself (its
//! format and version, one root, parents that exist, no loop) and for every path it names (a plain relative path that an entry of
//! the archive has, of the size the manifest says). The hash of each record is checked when the record is read, so that nothing is
//! trusted before it has matched what the manifest promised.

use super::model::{Opened, Plain, PlainFormat};
use super::source::{
    Entries, EntryFact, LooseEntries, Packing, PartsEntries, SourceError, TarEntries, ZipEntries,
    ZipFileEntries, clean_path, looks_like_zip,
};
use crate::engine::Engine;
use orch_api_contract::imports::{ImportLoss, ImportSource, ImportSourceMode};
use orch_api_contract::package::{
    PACKAGE_FORMAT, PACKAGE_MANIFEST, PACKAGE_VERSION, PackageKind, PackageManifest, PackageObject,
};
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::read_all;
use orch_types::{Hash, Id};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How large a manifest may be at the least, whatever the limit of things is: sixty-four mebibytes. What it may be is the larger of this and
/// [`MANIFEST_BYTES_PER_ITEM`] for each thing the place lets one import hold, so that raising `import.max_items` raises it.
pub const MANIFEST_FLOOR_BYTES: u64 = 64 * 1024 * 1024;

/// How many bytes of manifest each thing that an import may hold is allowed: a thing is written in about seven hundred, and one that
/// carries many files or versions in a few thousand more.
pub const MANIFEST_BYTES_PER_ITEM: u64 = 4_096;

/// How many entries of an archive an import looks into for each thing it may hold: a thing's record, its reading copy, its versions
/// and its files. It is a multiplier of the limit of things, so that raising `import.max_items` raises it.
pub const ENTRIES_PER_ITEM: u64 = 16;

/// How many entries an archive may claim at the least, whatever the limits are, for a small limit and a folder of notes.
pub const ENTRIES_MIN: u64 = 1_000;

/// How many bytes of an archive's expanded size one entry is counted for at the most: what the limit of bytes allows is never refused
/// for the number of entries it is cut into.
pub const BYTES_PER_ENTRY: u64 = 4_096;

/// What an import of this size may claim: the entries of an archive and the bytes of a manifest, from the limits of the place.
#[must_use]
pub fn claims(limits: &orch_api_contract::imports::ImportLimits) -> (u64, u64) {
    let items = u64::try_from(limits.max_items).unwrap_or(0);
    let bytes = u64::try_from(limits.max_bytes).unwrap_or(0);
    (
        items
            .saturating_mul(ENTRIES_PER_ITEM)
            .max(bytes / BYTES_PER_ENTRY)
            .max(ENTRIES_MIN),
        items
            .saturating_mul(MANIFEST_BYTES_PER_ITEM)
            .max(MANIFEST_FLOOR_BYTES),
    )
}

/// Why a source could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// What was uploaded is not what an import reads, or is damaged: the reason, in a sentence.
    #[error("{0}")]
    Invalid(String),
    /// Something the import stands on failed.
    #[error("{0}")]
    Failed(String),
}

impl From<SourceError> for OpenError {
    fn from(error: SourceError) -> Self {
        match error {
            SourceError::Failed(message) => Self::Failed(message),
            other => Self::Invalid(other.to_string()),
        }
    }
}

fn invalid(message: impl Into<String>) -> OpenError {
    OpenError::Invalid(message.into())
}

/// Whether a file that begins as a zip is to be unpacked: it is when it is called a zip, and, where the person said it is an Orch package, whatever it
/// is called. Anything else that begins as a zip (a Word file, an archive attached to a document among other files) is a file like the others.
fn is_archive(name: &str, mode: Option<ImportSourceMode>) -> bool {
    mode == Some(ImportSourceMode::Package) || name.to_ascii_lowercase().ends_with(".zip")
}

/// The endings of the name of a file that is a tar: `.tar`, and the short and the long names of the ones packed with gzip, bzip2, xz or zstd.
const TAR_ENDINGS: [&str; 10] = [
    ".tar", ".tgz", ".tar.gz", ".tbz", ".tbz2", ".tar.bz2", ".txz", ".tar.xz", ".tzst", ".tar.zst",
];

/// Whether a file is a tar to unpack, and how it is packed: it is a tar by its name, and it is packed as its first bytes say (a name that
/// says a packing over bytes that are not packed is no tar). A tar holds plain files; an Orch package is a zip.
fn tar_kind(name: &str, head: &[u8]) -> Option<Packing> {
    let lower = name.to_ascii_lowercase();
    if !TAR_ENDINGS.iter().any(|ending| lower.ends_with(ending)) {
        return None;
    }
    let packing = Packing::of(head);
    (lower.ends_with(".tar") || packing != Packing::Plain).then_some(packing)
}

/// Whether an entry of an archive is one that a Mac adds beside the files it archives, which is no file of the person's.
fn mac_litter(name: &str) -> bool {
    name.split('/')
        .any(|part| part == "__MACOSX" || part == ".DS_Store" || part.starts_with("._"))
}

/// The zips that an archive holds when it holds nothing else that can be imported: what a Notion export is, a zip of zips. An archive that holds
/// a text of any kind is a set of texts, and the zips in it are files like the others.
fn zips_inside(facts: &[EntryFact]) -> Vec<String> {
    let mut zips = Vec::new();
    for fact in facts {
        if fact.dir || mac_litter(&fact.name) {
            continue;
        }
        if plain_format(&fact.name).is_some() {
            return Vec::new();
        }
        if fact.name.to_ascii_lowercase().ends_with(".zip") {
            zips.push(fact.name.clone());
        }
    }
    zips
}

/// The parts of a source that is made of archives: so many entries in all as an import of this place may hold, and none of them a package.
fn check_parts(parts: &[Arc<dyn Entries>], most: u64) -> Result<(), OpenError> {
    let mut entries: u64 = 0;
    for part in parts {
        entries = entries.saturating_add(u64::try_from(part.facts().len()).unwrap_or(u64::MAX));
        if part
            .facts()
            .iter()
            .any(|fact| fact.name == PACKAGE_MANIFEST)
        {
            return Err(invalid("an Orch package is one zip archive"));
        }
    }
    if entries > most {
        return Err(OpenError::from(SourceError::TooManyEntries {
            found: entries,
            limit: most,
        }));
    }
    Ok(())
}

/// The zips `names` that the archive `outer` holds, each taken out into a scratch file and opened, as the parts of one source. What is taken
/// out is bounded by the bytes an import of this place may hold, together.
async fn zips_taken_out(
    engine: &Engine,
    outer: &Arc<dyn Entries>,
    names: &[String],
    most: u64,
    most_bytes: u64,
) -> Result<Vec<Arc<dyn Entries>>, OpenError> {
    let scratch = engine.scratch_dir();
    let mut parts: Vec<Arc<dyn Entries>> = Vec::with_capacity(names.len());
    let mut taken: u64 = 0;
    for name in names {
        let spooled = outer
            .spool(name, most_bytes.saturating_sub(taken), &scratch)
            .await?;
        taken = taken.saturating_add(spooled.size);
        parts.push(Arc::new(ZipFileEntries::open(spooled.path, most).await?));
    }
    check_parts(&parts, most)?;
    Ok(parts)
}

/// Opens the files of `source`. The limits of the place bound how many entries an archive may claim and how large its manifest may be.
pub async fn open(
    engine: &Engine,
    source: &ImportSource,
    limits: &orch_api_contract::imports::ImportLimits,
) -> Result<Opened, OpenError> {
    let Some(store) = engine.blobs() else {
        return Err(OpenError::Failed(
            "no store of content is attached, so nothing can be imported".to_owned(),
        ));
    };
    if source.files.is_empty() {
        return Err(invalid("select a package or files to import"));
    }
    let (most, manifest_most) = claims(limits);
    if u64::try_from(source.files.len()).unwrap_or(u64::MAX) > most {
        return Err(invalid(
            "more files are named than an import of this place may hold",
        ));
    }
    // What each file begins with says whether it is an archive.
    let mut archives = Vec::new();
    let mut tars = Vec::new();
    for file in &source.files {
        let size = store
            .size(&file.blob)
            .await
            .map_err(|error| OpenError::Failed(error.to_string()))?
            .ok_or_else(|| invalid(format!("{} is not there any more", file.name)))?;
        // The first eight bytes: a zip says what it is in four, an xz in six.
        let head = if size > 0 {
            let read = store
                .get(&file.blob, Some(ByteRange::Between { start: 0, end: 7 }))
                .await
                .map_err(|error| OpenError::Failed(error.to_string()))?;
            read_all(read.body)
                .await
                .map_err(|error| OpenError::Failed(error.to_string()))?
        } else {
            bytes::Bytes::new()
        };
        // Which files are archives is decided by what the person chose and what they are called before what they begin with: a Word file
        // or a file attached among others begins as a zip does and is not one to unpack.
        if looks_like_zip(&head) && is_archive(&file.name, source.mode) {
            archives.push((file, u64::try_from(size).unwrap_or(0)));
        } else if let Some(packing) = tar_kind(&file.name, &head) {
            tars.push((file, u64::try_from(size).unwrap_or(0), packing));
        }
    }
    let hashes: Vec<Hash> = source.files.iter().map(|file| file.blob).collect();
    if let [(file, size)] = archives.as_slice()
        && source.files.len() == 1
    {
        let zip = ZipEntries::open(Arc::clone(&store), file.blob, *size, most).await?;
        let entries: Arc<dyn Entries> = Arc::new(zip);
        let has_manifest = entries
            .facts()
            .iter()
            .any(|fact| fact.name == PACKAGE_MANIFEST);
        let package = source.mode == Some(ImportSourceMode::Package)
            || (has_manifest && source.mode != Some(ImportSourceMode::Files));
        if package {
            return package_of(entries, manifest_most).await;
        }
        // A zip of zips (a Notion export is one): the zips inside are the source, read as one.
        let inside = zips_inside(entries.facts());
        if !inside.is_empty() {
            let most_bytes = u64::try_from(limits.max_bytes).unwrap_or(0);
            let parts = zips_taken_out(engine, &entries, &inside, most, most_bytes).await?;
            return Ok(files_of(
                Arc::new(PartsEntries::new(parts)),
                &hashes,
                ImportSourceMode::Files,
            ));
        }
        return Ok(files_of(entries, &hashes, ImportSourceMode::Files));
    }
    // Several zips chosen together (the parts of a Notion export): one source.
    if archives.len() > 1
        && archives.len() == source.files.len()
        && source.mode != Some(ImportSourceMode::Package)
    {
        let mut parts: Vec<Arc<dyn Entries>> = Vec::with_capacity(archives.len());
        for (file, size) in &archives {
            parts.push(Arc::new(
                ZipEntries::open(Arc::clone(&store), file.blob, *size, most).await?,
            ));
        }
        check_parts(&parts, most)?;
        return Ok(files_of(
            Arc::new(PartsEntries::new(parts)),
            &hashes,
            ImportSourceMode::Files,
        ));
    }
    if let [(file, size, packing)] = tars.as_slice()
        && source.files.len() == 1
        && source.mode != Some(ImportSourceMode::Package)
    {
        // A tar has no index: it is expanded once into a scratch file, within the bytes the place lets an import hold.
        let most_bytes = u64::try_from(limits.max_bytes).unwrap_or(0);
        let tar = TarEntries::open(
            Arc::clone(&store),
            file.blob,
            *size,
            *packing,
            most,
            most_bytes,
            engine.scratch_dir(),
        )
        .await?;
        return Ok(files_of(Arc::new(tar), &hashes, ImportSourceMode::Files));
    }
    if source.mode == Some(ImportSourceMode::Package) {
        return Err(invalid("an Orch package is one zip archive"));
    }
    let named: Vec<(String, Hash)> = source
        .files
        .iter()
        .map(|file| (file.name.clone(), file.blob))
        .collect();
    let loose = LooseEntries::open(Arc::clone(&store), &named).await?;
    Ok(files_of(Arc::new(loose), &hashes, ImportSourceMode::Files))
}

// ---------------------------------------------------------------- a package

/// A package, with its manifest read and checked.
async fn package_of(entries: Arc<dyn Entries>, manifest_most: u64) -> Result<Opened, OpenError> {
    let facts: HashMap<&str, &EntryFact> = entries
        .facts()
        .iter()
        .map(|fact| (fact.name.as_str(), fact))
        .collect();
    let Some(manifest_fact) = facts.get(PACKAGE_MANIFEST) else {
        return Err(invalid(
            "this archive has no manifest.json: it is not an Orch package",
        ));
    };
    if manifest_fact.size > manifest_most {
        return Err(invalid(
            "the manifest of this package is larger than may be read",
        ));
    }
    let bytes = entries.read(PACKAGE_MANIFEST, manifest_most).await?;
    let manifest: PackageManifest = serde_json::from_slice(&bytes).map_err(|error| {
        invalid(format!(
            "the manifest of this package cannot be read: {error}"
        ))
    })?;
    if manifest.format != PACKAGE_FORMAT {
        return Err(invalid(
            "this archive's manifest is not that of an Orch package",
        ));
    }
    if manifest.version > PACKAGE_VERSION {
        return Err(invalid(format!(
            "this package is of version {}, and this server reads up to {PACKAGE_VERSION}",
            manifest.version
        )));
    }
    check_objects(&manifest.objects, &facts)?;
    let entry_count = i64::try_from(entries.facts().len()).unwrap_or(i64::MAX);
    let bytes = entries.facts().iter().fold(0_i64, |total, fact| {
        total.saturating_add(i64::try_from(fact.size).unwrap_or(i64::MAX))
    });
    Ok(Opened {
        mode: ImportSourceMode::Package,
        entries: Arc::clone(&entries),
        package: manifest.header.package_id,
        header: Some(manifest.header),
        objects: manifest.objects,
        principals: manifest.principals,
        access: manifest.access,
        left_out: manifest.left_out,
        entry_count,
        bytes,
        plain: HashMap::new(),
        entry_names: std::collections::HashSet::new(),
        notes: Vec::new(),
    })
}

/// What a manifest says of its objects must hold together: one root, parents that are objects, no loop, and every path a
/// plain relative path to an entry of the archive.
fn check_objects(
    objects: &[PackageObject],
    facts: &HashMap<&str, &EntryFact>,
) -> Result<(), OpenError> {
    let mut seen: HashSet<Id> = HashSet::with_capacity(objects.len());
    for object in objects {
        if !seen.insert(object.id) {
            return Err(invalid(format!("the manifest names {} twice", object.id)));
        }
    }
    let by_id: HashMap<Id, &PackageObject> =
        objects.iter().map(|object| (object.id, object)).collect();
    let roots = objects
        .iter()
        .filter(|object| object.parent.is_none())
        .count();
    if roots != 1 {
        return Err(invalid(format!(
            "the manifest has {roots} objects that lie in nothing, and a package has one"
        )));
    }
    for object in objects {
        if let Some(parent) = object.parent
            && !by_id.contains_key(&parent)
        {
            return Err(invalid(format!(
                "{} lies in {parent}, which the manifest does not list",
                object.name
            )));
        }
        // A loop: walking up never ends.
        let mut at = object.parent;
        let mut steps = 0_usize;
        while let Some(parent) = at {
            steps += 1;
            if steps > objects.len() || parent == object.id {
                return Err(invalid(format!("{} lies inside itself", object.name)));
            }
            at = by_id.get(&parent).and_then(|each| each.parent);
        }
        let paths = object
            .path
            .iter()
            .map(|path| (path, None))
            .chain(object.renderings.iter().map(|each| (&each.path, None)))
            .chain(
                object
                    .files
                    .iter()
                    .map(|each| (&each.path, Some(each.bytes))),
            )
            .chain(object.versions.iter().map(|each| (&each.path, None)));
        for (path, declared) in paths {
            let clean = clean_path(path)
                .map_err(|_| invalid(format!("{path} is not a path inside the package")))?;
            let Some(fact) = facts.get(clean.as_str()) else {
                return Err(invalid(format!(
                    "{path} is named by the manifest and is not in the archive"
                )));
            };
            if fact.dir {
                return Err(invalid(format!("{path} is a directory, not a file")));
            }
            if let Some(declared) = declared
                && i64::try_from(fact.size).unwrap_or(i64::MAX) != declared
            {
                return Err(invalid(format!("{path} is not the size the manifest says")));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- plain files

/// What a plain file is written in, by its extension.
#[must_use]
pub fn plain_format(name: &str) -> Option<PlainFormat> {
    let lower = name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.')?.1;
    match extension {
        "md" | "markdown" | "mdown" | "mkd" => Some(PlainFormat::Markdown),
        "html" | "htm" | "xhtml" => Some(PlainFormat::Html),
        "txt" | "text" => Some(PlainFormat::Text),
        "docx" => Some(PlainFormat::Word),
        _ => None,
    }
}

/// Notion appends a space and a 32-digit hexadecimal identity to display names. Paths keep it for links and identity.
fn plain_name(name: &str) -> String {
    let trimmed = name.trim();
    if let Some((title, suffix)) = trimmed.rsplit_once(char::is_whitespace)
        && !title.trim_end().is_empty()
        && suffix.len() == 32
        && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        title.trim_end().to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The name of a document made of the file `entry`, without its extension or export-only identity.
fn title_of(entry: &str) -> String {
    let name = entry.rsplit('/').next().unwrap_or(entry);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    plain_name(stem)
}

/// Reserve original sibling names first, then number collisions without querying or rescanning earlier siblings.
/// Returns the number of numbered candidates examined: each base advances its counter rather than starting again at two.
fn unique_plain_names(objects: &mut [PackageObject]) -> usize {
    let longest = orch_api_contract::documents::DOCUMENT_TITLE_MAX_CHARS as usize;
    // At most one original and one generated name per object can consume a number. A fixed stem
    // keeps long names which truncate alike on the same counter, rather than retrying all prior suffixes.
    let tail_room = format!(" ({})", objects.len().saturating_mul(2).saturating_add(2)).len();
    let mut used: HashMap<(Option<Id>, String), usize> = HashMap::new();
    let mut next: HashMap<(Option<Id>, String), usize> = HashMap::new();
    for object in objects.iter_mut() {
        let cut: String = object.name.chars().take(longest).collect();
        object.name = if cut.trim().is_empty() {
            if object.kind == PackageKind::Folder {
                "Folder"
            } else {
                "Untitled"
            }
            .to_owned()
        } else {
            cut.trim().to_owned()
        };
        used.entry((object.parent, object.name.to_lowercase()))
            .or_default();
    }
    let mut examined = 0_usize;
    for object in objects {
        let key = (object.parent, object.name.to_lowercase());
        if used.get(&key).copied().unwrap_or(0) == 0 {
            used.insert(key, 1);
            continue;
        }
        let stem: String = object
            .name
            .chars()
            .take(longest.saturating_sub(tail_room))
            .collect();
        let stem = stem.trim_end();
        let family = (object.parent, stem.to_lowercase());
        let number = next.entry(family).or_insert(2);
        loop {
            examined = examined.saturating_add(1);
            let tail = format!(" ({number})");
            let candidate = format!("{stem}{tail}");
            let candidate_key = (object.parent, candidate.to_lowercase());
            *number = number.saturating_add(1);
            if used.contains_key(&candidate_key) {
                continue;
            }
            used.insert(candidate_key, 1);
            object.name = candidate;
            break;
        }
    }
    examined
}

/// Plain files as objects: each folder of a name becomes a document folder, each file a document that can be read becomes a
/// document in the folder it lies in, and every other file is kept for what the documents name. The identifier of each is
/// made of its path, so that the same files imported again meet what they were made.
fn files_of(entries: Arc<dyn Entries>, hashes: &[Hash], mode: ImportSourceMode) -> Opened {
    let package = super::identity::files(hashes);
    let mut objects: Vec<PackageObject> = Vec::new();
    let mut plain: HashMap<Id, Plain> = HashMap::new();
    let mut notes: Vec<ImportLoss> = Vec::new();
    let mut folders: HashMap<String, Id> = HashMap::new();
    let destination_less = Id::nil();
    let mut folder_of = |path: &str, objects: &mut Vec<PackageObject>| -> Option<Id> {
        let mut parent: Option<Id> = None;
        let mut so_far = String::new();
        for part in path.split('/') {
            if !so_far.is_empty() {
                so_far.push('/');
            }
            so_far.push_str(part);
            let id = *folders.entry(so_far.clone()).or_insert_with(|| {
                let id = super::identity::path(destination_less, &format!("{so_far}/"));
                objects.push(folder_object(id, parent, part));
                id
            });
            parent = Some(id);
        }
        parent
    };
    let mut skipped = 0_i64;
    for fact in entries.facts() {
        // What a Mac adds beside the files it archives (`__MACOSX/._plan.md` is no Markdown) is no file of the person's.
        if fact.dir || mac_litter(&fact.name) {
            continue;
        }
        let Ok(name) = clean_path(&fact.name) else {
            skipped += 1;
            continue;
        };
        let Some(format) = plain_format(&name) else {
            continue;
        };
        let parent = name
            .rsplit_once('/')
            .and_then(|(directory, _)| folder_of(directory, &mut objects));
        let id = super::identity::path(destination_less, &name);
        objects.push(PackageObject {
            kind: PackageKind::Document,
            id,
            parent,
            path: Some(name.clone()),
            name: title_of(&name),
            description: String::new(),
            hidden: false,
            created_at: None,
            updated_at: None,
            creator: None,
            sha256: None,
            bytes: i64::try_from(fact.size).unwrap_or(i64::MAX),
            messages: None,
            turns: None,
            comments: None,
            renderings: Vec::new(),
            files: Vec::new(),
            versions: Vec::new(),
        });
        plain.insert(
            id,
            Plain {
                entry: name,
                format,
            },
        );
    }
    // Only names change: original paths and source identities still drive links and repeated imports.
    unique_plain_names(&mut objects);
    if skipped > 0 {
        notes.push(ImportLoss {
            code: "unsupported_block".to_owned(),
            count: skipped,
            message: format!("{skipped} files have names that cannot be paths and are left out"),
            object: None,
            params: None,
        });
    }
    let entry_names: std::collections::HashSet<String> = entries
        .facts()
        .iter()
        .filter(|fact| !fact.dir)
        .filter_map(|fact| clean_path(&fact.name).ok())
        .collect();
    let entry_count = i64::try_from(entries.facts().len()).unwrap_or(i64::MAX);
    let bytes = entries.facts().iter().fold(0_i64, |total, fact| {
        total.saturating_add(i64::try_from(fact.size).unwrap_or(i64::MAX))
    });
    Opened {
        mode,
        entries,
        header: None,
        objects,
        principals: Vec::new(),
        access: None,
        left_out: Vec::new(),
        package,
        entry_count,
        bytes,
        plain,
        entry_names,
        notes,
    }
}

fn folder_object(id: Id, parent: Option<Id>, name: &str) -> PackageObject {
    PackageObject {
        kind: PackageKind::Folder,
        id,
        parent,
        path: None,
        name: plain_name(name),
        description: String::new(),
        hidden: false,
        created_at: None,
        updated_at: None,
        creator: None,
        sha256: None,
        bytes: 0,
        messages: None,
        turns: None,
        comments: None,
        renderings: Vec::new(),
        files: Vec::new(),
        versions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn object(kind: PackageKind, id: Id, parent: Option<Id>) -> PackageObject {
        let mut object = folder_object(id, parent, "x");
        object.kind = kind;
        object
    }

    #[test]
    fn a_file_is_an_archive_by_its_name_or_by_the_persons_word_and_a_word_file_is_never_one() {
        assert!(is_archive("notes.zip", None));
        assert!(is_archive("NOTES.ZIP", Some(ImportSourceMode::Files)));
        assert!(
            !is_archive("report.docx", None),
            "a Word file begins as a zip and is a document"
        );
        assert!(!is_archive("attachment.dat", Some(ImportSourceMode::Files)));
        assert!(is_archive("package.bin", Some(ImportSourceMode::Package)));
    }

    #[test]
    fn a_file_is_a_tar_by_its_name_and_how_it_is_packed_by_what_it_begins_with() {
        let gzip = [0x1f, 0x8b, 8, 0];
        assert_eq!(tar_kind("notes.tar", b"abcd"), Some(Packing::Plain));
        assert_eq!(tar_kind("NOTES.TAR", b"abcd"), Some(Packing::Plain));
        assert_eq!(tar_kind("notes.tgz", &gzip), Some(Packing::Gzip));
        assert_eq!(tar_kind("notes.tar.gz", &gzip), Some(Packing::Gzip));
        assert_eq!(tar_kind("notes.tar.bz2", b"BZh9"), Some(Packing::Bzip2));
        assert_eq!(tar_kind("notes.tbz2", b"BZh9"), Some(Packing::Bzip2));
        assert_eq!(
            tar_kind("notes.txz", &[0xfd, b'7', b'z', b'X', b'Z', 0]),
            Some(Packing::Xz)
        );
        assert_eq!(
            tar_kind("notes.tar.zst", &[0x28, 0xb5, 0x2f, 0xfd]),
            Some(Packing::Zstd)
        );
        assert_eq!(
            tar_kind("notes.tar.gz", b"abcd"),
            None,
            "a name that says it is packed over bytes that are not is no tar"
        );
        assert_eq!(tar_kind("notes.md", &gzip), None);
        assert_eq!(tar_kind("notes.gz", &gzip), None);
    }

    #[test]
    fn a_zip_that_holds_nothing_but_zips_is_a_zip_of_zips_and_the_litter_of_a_mac_is_none_of_them()
    {
        let fact = |name: &str, dir: bool| EntryFact {
            name: name.to_owned(),
            size: 1,
            dir,
        };
        let facts = vec![
            fact("Export-1/", true),
            fact("Export-1/Part-1.zip", false),
            fact("Export-1/Part-2.ZIP", false),
            fact("__MACOSX/Export-1/._Part-1.zip", false),
            fact(".DS_Store", false),
        ];
        assert_eq!(
            zips_inside(&facts),
            ["Export-1/Part-1.zip", "Export-1/Part-2.ZIP"]
        );
        let mut with_text = facts.clone();
        with_text.push(fact("Export-1/readme.md", false));
        assert!(
            zips_inside(&with_text).is_empty(),
            "an archive that holds a text is a set of texts, and its zips are files like the others"
        );
        assert!(zips_inside(&[fact("a.png", false)]).is_empty());
        assert!(mac_litter("a/__MACOSX/b") && mac_litter("x/._y.md") && !mac_litter("x/y._z.md"));
    }

    #[test]
    fn what_a_source_may_claim_rises_with_the_limits_of_the_place_and_is_never_below_its_floors() {
        use orch_api_contract::imports::ImportLimits;
        let shipped = claims(&ImportLimits {
            max_bytes: 2_147_483_648,
            max_items: 10_000,
        });
        let raised = claims(&ImportLimits {
            max_bytes: 1_099_511_627_776,
            max_items: 1_000_000,
        });
        assert!(
            raised.0 > shipped.0 && raised.1 > shipped.1,
            "{shipped:?} {raised:?}"
        );
        assert!(
            raised.0 >= 16_000_000,
            "sixteen entries for each of a million things"
        );
        assert!(
            raised.1 >= 4_096_000_000,
            "four kibibytes of manifest for each of a million things"
        );
        let tiny = claims(&ImportLimits {
            max_bytes: 1,
            max_items: 1,
        });
        assert_eq!((tiny.0, tiny.1), (ENTRIES_MIN, MANIFEST_FLOOR_BYTES));
    }

    #[test]
    fn a_manifest_with_two_roots_a_missing_parent_or_a_loop_is_refused() {
        let (a, b, c) = (new_id(), new_id(), new_id());
        let none: HashMap<&str, &EntryFact> = HashMap::new();
        assert!(check_objects(&[object(PackageKind::Project, a, None)], &none).is_ok());
        assert!(
            check_objects(
                &[
                    object(PackageKind::Project, a, None),
                    object(PackageKind::Project, b, None)
                ],
                &none
            )
            .is_err(),
            "two things that lie in nothing"
        );
        assert!(
            check_objects(
                &[
                    object(PackageKind::Project, a, None),
                    object(PackageKind::Topic, b, Some(c))
                ],
                &none
            )
            .is_err(),
            "a parent the manifest does not list"
        );
        assert!(
            check_objects(
                &[
                    object(PackageKind::Project, a, None),
                    object(PackageKind::Topic, b, Some(c)),
                    object(PackageKind::Topic, c, Some(b)),
                ],
                &none
            )
            .is_err(),
            "two that lie in each other"
        );
        assert!(
            check_objects(
                &[
                    object(PackageKind::Project, a, None),
                    object(PackageKind::Topic, a, Some(a))
                ],
                &none
            )
            .is_err(),
            "one named twice"
        );
    }

    #[test]
    fn a_path_a_manifest_names_has_to_be_in_the_archive_at_the_size_it_says() {
        let a = new_id();
        let mut root = object(PackageKind::Project, a, None);
        root.path = Some("Web/readme.md".to_owned());
        let present = EntryFact {
            name: "Web/readme.md".to_owned(),
            size: 4,
            dir: false,
        };
        let mut facts: HashMap<&str, &EntryFact> = HashMap::new();
        assert!(check_objects(std::slice::from_ref(&root), &facts).is_err());
        facts.insert("Web/readme.md", &present);
        assert!(check_objects(std::slice::from_ref(&root), &facts).is_ok());
        root.path = Some("../outside.md".to_owned());
        assert!(check_objects(&[root], &facts).is_err());
    }

    #[test]
    fn a_file_is_a_document_by_its_extension_and_the_title_is_its_name() {
        assert_eq!(plain_format("Notes/a.MD"), Some(PlainFormat::Markdown));
        assert_eq!(plain_format("a.html"), Some(PlainFormat::Html));
        assert_eq!(plain_format("a.docx"), Some(PlainFormat::Word));
        assert_eq!(plain_format("image.png"), None);
        assert_eq!(plain_format("noextension"), None);
        assert_eq!(title_of("Notes/Plan v2.md"), "Plan v2");
    }

    #[test]
    fn only_a_trailing_notion_identity_is_removed_from_a_display_name() {
        const ID: &str = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";
        assert_eq!(ID.len(), 32);
        assert_eq!(title_of(&format!("Notes/Roadmap {ID}.md")), "Roadmap");
        assert_eq!(
            plain_name(&format!("里程碑 {}", ID.to_uppercase())),
            "里程碑"
        );
        assert_eq!(plain_name(&format!(" {ID}")), ID);
        assert_eq!(plain_name("Roadmap 1a2b"), "Roadmap 1a2b");
        assert_eq!(
            plain_name("Roadmap 0g1b2c3d4e5f60718293a4b5c6d7e8f9"),
            "Roadmap 0g1b2c3d4e5f60718293a4b5c6d7e8f9"
        );
        assert_eq!(
            plain_name("Roadmap 0a1b2c3d4e5f60718293a4b5c6d7e8f90"),
            "Roadmap 0a1b2c3d4e5f60718293a4b5c6d7e8f90"
        );
        assert_eq!(
            plain_name(&format!("{ID} Roadmap")),
            format!("{ID} Roadmap")
        );
    }

    #[test]
    fn cleaned_sibling_names_stay_unique_without_changing_paths_parents_or_source_ids() {
        let parent = Id::from_u128(1);
        let other = Id::from_u128(2);
        let mut objects = [
            folder_object(
                Id::from_u128(3),
                Some(parent),
                "Roadmap 0a1b2c3d4e5f60718293a4b5c6d7e8f9",
            ),
            folder_object(
                Id::from_u128(4),
                Some(parent),
                "Roadmap 1a1b2c3d4e5f60718293a4b5c6d7e8f9",
            ),
            folder_object(Id::from_u128(5), Some(parent), "Roadmap (2)"),
            folder_object(Id::from_u128(6), Some(other), "Roadmap"),
        ];
        objects[0].path = Some("original 0a1b2c3d4e5f60718293a4b5c6d7e8f9/".to_owned());
        let identity: Vec<_> = objects
            .iter()
            .map(|object| (object.id, object.parent, object.path.clone()))
            .collect();
        unique_plain_names(&mut objects);
        assert_eq!(
            objects
                .iter()
                .map(|object| object.name.as_str())
                .collect::<Vec<_>>(),
            ["Roadmap", "Roadmap (3)", "Roadmap (2)", "Roadmap"]
        );
        assert_eq!(
            objects
                .iter()
                .map(|object| (object.id, object.parent, object.path.clone()))
                .collect::<Vec<_>>(),
            identity
        );
    }

    #[test]
    fn folders_and_documents_share_one_name_space_after_notions_suffix_is_removed() {
        let parent = Id::from_u128(10);
        let mut folder = folder_object(Id::from_u128(1), Some(parent), "Roadmap");
        let mut document = folder_object(Id::from_u128(2), Some(parent), "Roadmap");
        document.kind = PackageKind::Document;
        let reserved = folder_object(Id::from_u128(3), Some(parent), "Roadmap (2)");
        folder.path = Some("original-folder/".to_owned());
        document.path = Some("original-document.md".to_owned());
        let mut objects = [folder, document, reserved];
        unique_plain_names(&mut objects);
        assert_eq!(
            objects
                .iter()
                .map(|object| object.name.as_str())
                .collect::<Vec<_>>(),
            ["Roadmap", "Roadmap (3)", "Roadmap (2)"]
        );
        assert_eq!(objects[0].path.as_deref(), Some("original-folder/"));
        assert_eq!(objects[1].path.as_deref(), Some("original-document.md"));
    }

    #[test]
    fn a_hundred_thousand_equal_cleaned_names_use_a_linear_number_of_candidates() {
        let mut objects: Vec<_> = (0..100_000_u128)
            .map(|index| folder_object(Id::from_u128(index + 1), None, "Roadmap"))
            .collect();
        let examined = unique_plain_names(&mut objects);
        assert_eq!(examined, objects.len() - 1);
        assert_eq!(objects.first().unwrap().name, "Roadmap");
        assert_eq!(objects.last().unwrap().name, "Roadmap (100000)");
        let names: HashSet<_> = objects
            .iter()
            .map(|object| object.name.to_lowercase())
            .collect();
        assert_eq!(names.len(), objects.len());
    }

    #[test]
    fn a_hundred_thousand_long_names_that_truncate_alike_do_not_retry_prior_numbers() {
        let prefix = "x".repeat(196);
        let mut objects: Vec<_> = (0..100_000_u128)
            .map(|index| {
                folder_object(
                    Id::from_u128(index + 1),
                    None,
                    &format!("{prefix}{:04x}", index / 2),
                )
            })
            .collect();
        let examined = unique_plain_names(&mut objects);
        assert_eq!(examined, objects.len() / 2);
        let names: HashSet<_> = objects
            .iter()
            .map(|object| object.name.to_lowercase())
            .collect();
        assert_eq!(names.len(), objects.len());
        assert!(
            objects
                .iter()
                .all(|object| object.name.chars().count() <= 200)
        );
    }
}
