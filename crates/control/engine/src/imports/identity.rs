//! The identifiers an import gives: each is made from what it stands for, so that a retry makes the same ones and nothing twice.
//!
//! An object an import makes gets the identifier derived from who imports, the place imported into, the package and the
//! identifier the object had in its source. What an import adds to something that was there already (a message, a turn, a
//! comment) gets one derived from the thing it is added to and its own source identifier, whichever package it came in, so
//! that the same message in a later package is the one that was brought. Each is a domain-separated, length-framed BLAKE3 of
//! its parts, cut to the 128 bits of an identifier, as a copy of a document makes its own.

use orch_types::{Hash, Id};

/// The domain of an object an import makes.
const OBJECT_DOMAIN: &[u8] = b"import-object-v1";
/// A replacement incarnation must leave the removed object's identifier restorable.
const RECREATED_OBJECT_DOMAIN: &[u8] = b"import-recreated-object-v1";
/// The domain of a row an import adds to something that was there.
const INSIDE_DOMAIN: &[u8] = b"import-inside-v1";
/// The domain of a block an import writes in a document.
const BLOCK_DOMAIN: &[u8] = b"import-block-v1";
/// The domain of a file an import puts in a document.
const FILE_DOMAIN: &[u8] = b"import-file-v1";
/// The domain of the identifier a plain file has as the source of its own.
const PATH_DOMAIN: &[u8] = b"import-path-v1";
/// The domain of the identifier a set of files has as a package of its own.
const FILES_DOMAIN: &[u8] = b"import-files-v1";

fn framed(domain: &[u8], parts: &[&[u8]]) -> Id {
    let mut hash = Hash::hasher();
    for part in std::iter::once(&domain).chain(parts.iter()) {
        hash.update(&u64::try_from(part.len()).unwrap_or(u64::MAX).to_le_bytes());
        hash.update(part);
    }
    let digest = Hash::finish(&hash);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    // Version 8, variant 1 (custom): the identifier says it was made by hashing and not drawn at random.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

/// The identifier an object of a source gets here.
#[must_use]
pub fn object(importer: Id, destination: Id, package: Id, source: Id) -> Id {
    framed(
        OBJECT_DOMAIN,
        &[
            importer.as_bytes(),
            destination.as_bytes(),
            package.as_bytes(),
            source.as_bytes(),
        ],
    )
}

/// The stable next incarnation after an earlier imported object was removed.
#[must_use]
pub fn recreated(object: Id, removed: Id) -> Id {
    framed(
        RECREATED_OBJECT_DOMAIN,
        &[object.as_bytes(), removed.as_bytes()],
    )
}

/// The identifier a row of a source gets when it is added to `parent` here (a message of a chat, a block of a document, a
/// file of a document): the same whichever package it came in.
#[must_use]
pub fn inside(parent: Id, kind: &str, source: Id) -> Id {
    framed(
        INSIDE_DOMAIN,
        &[parent.as_bytes(), kind.as_bytes(), source.as_bytes()],
    )
}

/// The identifier of the block `source` of a document, written into the document `document` here by the import `import`:
/// not the same in another import, since what a replaced document held before has its own blocks, gone but not forgotten.
#[must_use]
pub fn block(import: Id, document: Id, source: Id) -> Id {
    framed(
        BLOCK_DOMAIN,
        &[import.as_bytes(), document.as_bytes(), source.as_bytes()],
    )
}

/// The identifier of the file `source` of a document, put into the document `document` here by the import `import`.
#[must_use]
pub fn file(import: Id, document: Id, source: Id) -> Id {
    framed(
        FILE_DOMAIN,
        &[import.as_bytes(), document.as_bytes(), source.as_bytes()],
    )
}

/// The source identifier of the plain file at `path` of a source that is imported into `destination`.
#[must_use]
pub fn path(destination: Id, path: &str) -> Id {
    framed(PATH_DOMAIN, &[destination.as_bytes(), path.as_bytes()])
}

/// The identifier of the package that a set of plain files is, from the hashes of what each holds.
#[must_use]
pub fn files(hashes: &[Hash]) -> Id {
    let mut sorted: Vec<[u8; 32]> = hashes.iter().map(|hash| *hash.as_bytes()).collect();
    sorted.sort_unstable();
    let parts: Vec<&[u8]> = sorted.iter().map(<[u8; 32]>::as_slice).collect();
    framed(FILES_DOMAIN, &parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn the_same_things_make_the_same_identifier_and_any_other_makes_another() {
        let (importer, place, package, source) = (new_id(), new_id(), new_id(), new_id());
        let first = object(importer, place, package, source);
        assert_eq!(first, object(importer, place, package, source));
        assert_ne!(first, object(new_id(), place, package, source));
        assert_ne!(first, object(importer, new_id(), package, source));
        assert_ne!(first, object(importer, place, new_id(), source));
        assert_ne!(first, object(importer, place, package, new_id()));
    }

    #[test]
    fn a_row_added_to_a_thing_is_the_same_whichever_package_brought_it() {
        let (chat, message) = (new_id(), new_id());
        assert_eq!(
            inside(chat, "message", message),
            inside(chat, "message", message)
        );
        assert_ne!(
            inside(chat, "message", message),
            inside(chat, "turn", message)
        );
        assert_ne!(
            inside(chat, "message", message),
            inside(new_id(), "message", message)
        );
    }

    #[test]
    fn recreating_removed_imports_keeps_retries_stable_and_each_incarnation_distinct() {
        let first = object(new_id(), new_id(), new_id(), new_id());
        let second = recreated(first, first);
        assert_eq!(second, recreated(first, first));
        assert_ne!(first, second);
        let third = recreated(first, second);
        assert_eq!(third, recreated(first, second));
        assert_ne!(first, third);
        assert_ne!(second, third);
        assert_ne!(second, recreated(new_id(), first));
    }

    #[test]
    fn the_parts_are_framed_so_that_moving_a_byte_between_them_changes_the_identifier() {
        let place = new_id();
        assert_ne!(path(place, "a/b"), path(place, "ab/"));
        assert_ne!(path(place, "a"), path(new_id(), "a"));
    }

    #[test]
    fn a_set_of_files_is_the_same_package_in_whatever_order_it_is_given() {
        let (one, two) = (Hash::of(b"one"), Hash::of(b"two"));
        assert_eq!(files(&[one, two]), files(&[two, one]));
        assert_ne!(files(&[one]), files(&[one, two]));
    }
}
