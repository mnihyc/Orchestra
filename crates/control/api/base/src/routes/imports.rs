//! Imports over HTTP: what a person brings into a place from an Orch package or from plain files, planned before anything is made, asked for as a job, listed, read with its report and taken back as a whole, all of it its person's alone.
//!
//! What an import does, and for whom, is the engine's ([`orch_engine::imports`]): each thing is made by its person's own rights
//! at the moment it is made. Here a request is read, the uploads it names are checked to be the person's own, an answer is made of
//! the engine's, and a refusal becomes the problem a client acts on: `import.too_large` and `import.too_many` with the figure and the
//! limit, `import.nothing`, `422 import.invalid` for a source that is not a package or files that can be read, `429` past what one
//! person may have running, and `409 import.undone` for an import that is taken back already.

orch_api_base::api_family_callback! {
    bind __api_bind_9586d57c59, __api_install_9586d57c59, __api_callback_9586d57c59;
    slot __ApiCallback9586d57c59, __API_CALLBACK_9586D57C59, __api_set_9586d57c59;
    base orch_api_base::routes::imports;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
