//! Documents: listing beside a node, blocks and their operations, comments, versions and what changed between two of them, files, export as Markdown, HTML and Typst with what it is asked to hold, import, moving and deleting.
//!
//! A document is a page of blocks written beside a node. This module makes
//! one (from blocks, or from Markdown, which is the way in for a script and
//! for an import), lists those beside a node that the caller may read, reads
//! one whole or what changed in it since a sequence, renames it and changes
//! its description, says how far the caller has had it on screen, moves it to
//! another node of its tree, and
//! deletes it and brings it back. The submodules hold the rest: `ops`
//! applies batches of changes, `history` lists versions, says what changed
//! between two and puts one back, `comments` holds the discussions beside
//! the text, `export` writes a document out, `files` puts files into it and
//! gives them back, and [`page`] is its permission page. Who may do what is
//! the access engine's, asked through [`crate::middleware::authorization`];
//! what a document may hold where it stands is `limits`; the names a reader
//! is shown are put in by `build`.
//!
//! Every change is one transaction that stamps the document from its own
//! sequence under the lock on its row and announces it when it commits, which
//! is how every open copy of a document stays the same document.

orch_api_base::api_family_callback! {
    bind __api_bind_00fe765ca7, __api_install_00fe765ca7, __api_callback_00fe765ca7;
    slot __ApiCallback00fe765ca7, __API_CALLBACK_00FE765CA7, __api_set_00fe765ca7;
    base orch_api_base::routes::documents;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

#[path = "documents/build.rs"]
pub mod build;

#[path = "documents/export.rs"]
pub mod export;

#[path = "documents/limits.rs"]
pub mod limits;

#[path = "documents/metadata_projection.rs"]
pub mod metadata_projection;

#[path = "documents/page.rs"]
pub mod page;

#[path = "documents/read_operation.rs"]
pub mod read_operation;

#[path = "documents/references.rs"]
pub mod references;
