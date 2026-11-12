//! A document written out as a file: Markdown, one HTML page, Typst source with what it names, or its blocks as JSON, with the files and the discussions beside it when they are asked for.
//!
//! Every renderer is a pure function of the document as it stands; here the
//! names a reader would see are put in, the parts that were asked for are
//! gathered, and the answer is a download: one file, or a zip when the
//! document has files to go beside it. Markdown says as much of a document
//! as Markdown can, and what it could not keep is told in the answer's
//! `X-Orch-Export-Losses` header and in the audit row of the export. A PDF is
//! made in the reader's browser from the Typst export, so that no service of
//! Orch compiles a document.
//!
//! The files a document holds are kept in the blob store. A Markdown export that has files to go beside it is a zip
//! of the text and of them, and the text links each file by the path it lies at in the zip, `<title>.files/<name>`
//! (see `orch_engine::written::files_beside`: the one rule of such a link, which an archive of a place and what an
//! agent is given follow as well). An HTML page holds its images inside it, as data addresses, so that it needs
//! nothing else; with the attachments asked for it is a zip of the page and of them, at `files/<name>`, the path the
//! page links them by. Typst source is always a zip: of the source, of the images it uses at the paths it names them
//! by, and of the attachments at `files/<name>` when they are asked for. A file whose bytes the store lost is left
//! out, and the page says that an image is not there.

orch_api_base::api_family_callback! {
    bind __api_bind_cfb69bd732, __api_install_cfb69bd732, __api_callback_cfb69bd732;
    slot __ApiCallbackcfb69bd732, __API_CALLBACK_CFB69BD732, __api_set_cfb69bd732;
    base orch_api_base::routes::documents::export;

    async fn single_form<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: &'__api orch_engine::exports::DocumentHow, arg4: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<(Vec<u8>, String)>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2b048ad610, __api_install_2b048ad610, __api_callback_2b048ad610;
    slot __ApiCallback2b048ad610, __API_CALLBACK_2B048AD610, __api_set_2b048ad610;
    base orch_api_base::routes::documents::export;

    async fn form_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: bool, arg4: bool) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_engine::exports::FormReference>>;
}
