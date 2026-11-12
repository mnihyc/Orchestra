//! Handing a resource or a document over to another person, who is its first owner from then on.
//!
//! One of the owners or an admin of the place the thing is in names the person in the Owners row and
//! says that they are first; the person who was first stays an owner unless the same request takes
//! them out. It is the engine's change of the Owners row, in one transaction, audited as the passing
//! on of the kind of thing it is, and the answer is the thing as its own request reads it. A node is
//! handed over by its own request; a kind that has no page here is answered plainly as not there.

orch_api_base::api_family_callback! {
    bind __api_bind_26428bb37a, __api_install_26428bb37a, __api_callback_26428bb37a;
    slot __ApiCallback26428bb37a, __API_CALLBACK_26428BB37A, __api_set_26428bb37a;
    base orch_api_base::routes::transfer;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}
