//! Exports over HTTP: an archive of what a person may read in a place, planned before anything is made, asked for as a job, listed, fetched and removed by its person alone.
//!
//! What goes in an archive, and for whom, is the engine's ([`orch_engine::exports`]): each thing is written by its
//! person's Export row at the moment it is written. Here a request is read, an answer is made of the engine's, and
//! a refusal becomes the problem a client acts on: `export.too_large` and `export.too_many` with the figure and the
//! limit, `export.nothing`, `429` past what one person may have being written, and `410` for an archive that was
//! removed. The two forms of a single export that need the views of the API (a document with the names a reader is
//! given, a thread as the API shows it) are written by [`ApiForms`], which the engine asks, so that a thing in an
//! archive is the very bytes of its single export.

use crate::routes::documents::export::single_form;

use async_trait::async_trait;

use chrono::DateTime;

use chrono::Utc;

use orch_api_contract::nodes::Node;

pub use orch_api_contract::threads::Thread;

use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_db::Conn;

use orch_engine::exports::DocumentForm;

use orch_engine::exports::DocumentHow;

use orch_engine::exports::Forms;

pub use orch_engine::exports::FormsFault;

pub use orch_types::Id;
orch_api_base::api_family_callback! {
    bind __api_bind_1047e2b49e, __api_install_1047e2b49e, __api_callback_1047e2b49e;
    slot __ApiCallback1047e2b49e, __API_CALLBACK_1047E2B49E, __api_set_1047e2b49e;
    base orch_api_base::routes::exports;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

// ---------------------------------------------------------------- what only this layer writes

/// Writes the forms of a single export that need the views of the API, for the engine: the same code that answers
/// a person's request for one thing.
#[derive(Debug, Default, Clone, Copy)]
pub struct ApiForms;

orch_api_base::api_family_callback! {
    bind __api_bind_351ca3187e, __api_install_351ca3187e, __api_callback_351ca3187e;
    slot __ApiCallback351ca3187e, __API_CALLBACK_351CA3187E, __api_set_351ca3187e;
    base orch_api_base::routes::exports;

    fn fault<'__api>(arg0: &'__api orch_api_base::middleware::requests::ApiError) -> orch_engine::exports::FormsFault;
}

#[async_trait]
impl Forms for ApiForms {
    async fn sources(
        &self,
        conn: &mut Conn,
        asker: &Principal,
        form: orch_engine::exports::Form,
    ) -> Result<Option<orch_engine::exports::FormSources>, FormsFault> {
        use orch_access_contract::permissions::ObjectKind;
        use orch_engine::exports::{Form, FormReference, FormSources};
        let references = match form {
            Form::Thread { id } => {
                let prepared = crate::routes::nodes::content_sources::prepare(conn, asker, id)
                    .await
                    .map_err(|error| fault(&error))?;
                let side = orch_engine::views::thread_metadata_sources(conn, id)
                    .await
                    .map_err(|error| FormsFault(error.to_string()))?
                    .and_then(|source| source.side_of);
                prepared
                    .objects
                    .into_iter()
                    .map(|(object, id)| {
                        if side == Some(id)
                            || matches!(
                                object,
                                ObjectKind::Agent
                                    | ObjectKind::Provider
                                    | ObjectKind::Machine
                                    | ObjectKind::Host
                                    | ObjectKind::Environment
                            )
                        {
                            FormReference::Published { object, id }
                        } else {
                            FormReference::Object { object, id }
                        }
                    })
                    .collect()
            }
            Form::Document { id, json, comments } => {
                crate::routes::documents::export::form_sources(conn, asker, id, json, comments)
                    .await
                    .map_err(|error| fault(&error))?
            }
        };
        Ok(Some(FormSources { references }))
    }
    async fn thread(
        &self,
        conn: &mut Conn,
        asker: &Principal,
        thread: Id,
        now: DateTime<Utc>,
    ) -> Result<Thread, FormsFault> {
        match crate::routes::nodes::view::one(conn, asker, thread, now, Policy::default()).await {
            Ok(Some(Node::Thread(thread))) => Ok(thread),
            Ok(_) => Err(FormsFault("the thread is gone".to_owned())),
            Err(error) => Err(fault(&error)),
        }
    }

    async fn document(
        &self,
        conn: &mut Conn,
        asker: &Principal,
        document: Id,
        how: DocumentHow,
        now: DateTime<Utc>,
    ) -> Result<DocumentForm, FormsFault> {
        single_form(conn, asker, document, &how, now)
            .await
            .map(|(bytes, losses)| DocumentForm { bytes, losses })
            .map_err(|error| fault(&error))
    }
}
