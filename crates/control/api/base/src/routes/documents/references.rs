//! The card a reference's chip opens: what the reference names, and the part it names, as the caller may read them.
//!
//! A chip in a document, in a chat or in what an agent wrote names a thing (a place, a thread, a document, a chat)
//! and may name one of its parts (a block, a turn, a message). Its card asks here once, when it opens: the thing's
//! name, where it lies, the first line of what it is about, when it last changed and who changed it, and the part
//! itself. Each is read with the checks of its own reading: `doc.view` on a document, `node.view` on a place, also
//! `thread.view` for a turn, and the chat's own reading for a chat. A caller who may not read the thing is answered
//! as if nothing were there, with no word of what it is: their chip is plain text already.

orch_api_base::api_family_callback! {
    bind __api_bind_22d35a3720, __api_install_22d35a3720, __api_callback_22d35a3720;
    slot __ApiCallback22d35a3720, __API_CALLBACK_22D35A3720, __api_set_22d35a3720;
    base orch_api_base::routes::documents::references;

    fn chat_files(arg0: Vec<orch_api_contract::chats::ChatPart>) -> Vec<(orch_types::Hash, String, Option<i64>, String)>;
}
