//! The text of an edit when it is a file of the workspace: read through the machine, made ready for the one reader, with the pictures and files it names read from the workspace beside it.
//!
//! `edit_document` takes `file` (a workspace path) and `format` in place of `markdown`. The file is read whole by the
//! shared bounded workspace reader, in whatever format its ending or `format` says (Markdown, HTML, Word, text); a
//! Markdown file that was checked out of a document says which document and at which sequence in its front matter.
//! The relative paths the text names (`![x](images/a.png)`, `[data](data.csv)`) are read from the workspace too, from
//! the directory the file lies in: those found become files of the document and the blocks that show them, and those
//! not found are listed in the answer and stay as words. Nothing is read from outside the thread's directory.

use std::collections::HashMap;

use orch_documents::checkout::{self, Checkout};
use orch_documents::convert::{self, Format, Prepared};
use orch_documents::markdown::TargetKind;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, new_id};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::tools::definitions::{Code, Refusal};
use crate::tools::workspace_files::{WorkspaceFile, path_refusal, relative_file};
use orch_db::queries::threads::ThreadRow;

/// What a source may expand to where no setting says: the shipped bound of a person's import.
const SOURCE_BYTES_DEFAULT: i64 = 64 * 1024 * 1024;
/// How many of the files a text names are read from the workspace: the rest are listed as missing, and a text that
/// names more is imported a part at a time.
pub(super) const FOUND_MAX: usize = 1_000;

/// The endings of files that are documents themselves: a link to one is a link, not a file to hold.
const DOCUMENT_ENDINGS: [&str; 7] = ["md", "markdown", "html", "htm", "xhtml", "docx", "txt"];

/// A file of the workspace that the text names, read.
#[derive(Debug, Clone)]
pub(crate) struct Found {
    /// The identifier its blocks name it by.
    pub(crate) id: Id,
    /// The name it is kept under in the document.
    pub(crate) name: String,
    /// Its bytes, in the store.
    pub(crate) file: WorkspaceFile,
}

/// The source of an edit, ready for the reader.
pub(crate) struct Imported {
    /// The source, as the reader holds it before it makes blocks.
    pub(crate) prepared: Prepared,
    /// The text of a Markdown source, after its front matter: what references and mentions are looked up in.
    pub(crate) text: String,
    /// What the front matter said, where there was one.
    pub(crate) checkout: Option<Checkout>,
    /// The directory the file lies in, in the workspace; the paths the text names are relative to it.
    pub(crate) base: String,
    /// The files it names that were found in the workspace, by their path there.
    pub(crate) found: HashMap<String, Found>,
}

fn bad_format(file: &str) -> Refusal {
    Refusal::invalid(format!(
        "{file:?} does not say what it is written in: give \"format\" as markdown, html, docx or text."
    ))
}

impl Engine {
    /// Reads the file an edit is made from, and the files it names, from the thread's workspace.
    pub(crate) async fn read_source(
        &self,
        thread: &ThreadRow,
        file: &str,
        format: Option<&str>,
    ) -> Result<Result<Imported, Refusal>, EngineError> {
        if let Some(refusal) = path_refusal(file) {
            return Ok(Err(refusal));
        }
        let format = match format {
            Some(word) => match Format::of_word(word) {
                Some(format) => format,
                None => {
                    return Ok(Err(Refusal::invalid(
                        "\"format\" is markdown, html, docx or text.",
                    )));
                }
            },
            None => match Format::of_name(file) {
                Some(format) => format,
                None => return Ok(Err(bad_format(file))),
            },
        };
        // The file is read within what an import may expand to, and the files it names within what a document's file may be.
        let (source_limit, file_limit) = {
            let mut conn = self.inner.db.acquire().await?;
            let place = Place::Node(thread.node_id);
            (
                orch_settings::get::<i64>(&mut conn, place, keys::SHARING_SNAPSHOT_MAX_BYTES)
                    .await?
                    .unwrap_or(SOURCE_BYTES_DEFAULT),
                orch_settings::get::<i64>(&mut conn, place, keys::DOCS_FILE_MAX_BYTES)
                    .await?
                    .unwrap_or(i64::MAX),
            )
        };
        let main = match self
            .read_workspace_files(thread, &[file.to_owned()], source_limit)
            .await?
        {
            Ok(mut files) => files
                .pop()
                .ok_or_else(|| EngineError::unreadable("the workspace reader returned no file"))?,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let bytes = match self.workspace_bytes(&main).await {
            Ok(bytes) => bytes,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let (text, checkout, bytes) = if format == Format::Markdown {
            let Ok(whole) = String::from_utf8(bytes) else {
                return Ok(Err(Refusal::invalid(format!(
                    "{file:?} is not UTF-8 text."
                ))));
            };
            let (checkout, body) = checkout::read(&whole);
            let text = body.to_owned();
            (text.clone(), checkout, text.into_bytes())
        } else {
            (String::new(), None, bytes)
        };
        let prepared =
            match tokio::task::spawn_blocking(move || convert::prepare(format, &bytes)).await {
                Ok(Ok(prepared)) => prepared,
                Ok(Err(error)) => {
                    return Ok(Err(Refusal::invalid(format!(
                        "{file:?} could not be read as {format:?}: {error}"
                    ))));
                }
                Err(error) => return Err(EngineError::unreadable(error.to_string())),
            };
        let base = file
            .rsplit_once('/')
            .map_or("", |(base, _)| base)
            .to_owned();
        let mut found: HashMap<String, Found> = HashMap::new();
        for target in prepared.targets() {
            let Some(path) = convert::join_path(&base, &target.path) else {
                continue;
            };
            if found.contains_key(&path) || !relative_file(&path) {
                continue;
            }
            if found.len() >= FOUND_MAX {
                break;
            }
            let ending = path
                .rsplit_once('.')
                .map_or(String::new(), |(_, ending)| ending.to_ascii_lowercase());
            if target.kind == TargetKind::Link && DOCUMENT_ENDINGS.contains(&ending.as_str()) {
                continue;
            }
            // A file that cannot be read is a path that leads nowhere, which the reader says.
            if let Ok(mut files) = self
                .read_workspace_files(thread, std::slice::from_ref(&path), file_limit)
                .await?
                && let Some(read) = files.pop()
            {
                let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                found.insert(
                    path,
                    Found {
                        id: new_id(),
                        name,
                        file: read,
                    },
                );
            }
        }
        Ok(Ok(Imported {
            prepared,
            text,
            checkout,
            base,
            found,
        }))
    }

    /// The bytes of a file that the workspace reader put in the store.
    pub(crate) async fn workspace_bytes(&self, file: &WorkspaceFile) -> Result<Vec<u8>, Refusal> {
        let closed = || {
            Refusal::new(
                Code::Closed,
                "The file store could not be read. Nothing was changed. Try again.",
            )
        };
        let store = self.blobs().ok_or_else(closed)?;
        let read = store.get(&file.blob, None).await.map_err(|_| closed())?;
        let bytes = orch_blob_contract::store::read_all(read.body)
            .await
            .map_err(|_| closed())?;
        if i64::try_from(bytes.len()).ok() != Some(file.size) {
            return Err(crate::tools::workspace_files::changed());
        }
        Ok(bytes.to_vec())
    }
}
