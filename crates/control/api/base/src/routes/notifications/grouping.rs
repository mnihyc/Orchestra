//! The Inbox by place: the same notifications grouped under the nearest place the caller may read, what they can act on first, paged under a fixed cut; a group marked read up to its cut, and undone by a sealed ticket.
//!
//! **Groups.** A notification is grouped under the nearest project on its path that the caller
//! may read now, else the nearest org they may read, else the place it is about where they may
//! read that; what is about the person alone (no place, no chat) is under Other. A place the
//! caller may not read is never named and never counted. Groups with something the caller can
//! act on now come first, then the group with the newest row, then the group's key; within a
//! group what waits for the caller comes first, then the newest. A list is paged under a cut of
//! the caller's Inbox sequence, given in the cursor, so that rows that arrive while it is paged
//! do not move it; counts are those of the whole group under the cut and the filters, never sums
//! of pages. A list asked for again after access or a decision changed starts again.
//!
//! **Reading a group.** The group key and its cut name what is marked: the rows that are unread,
//! at or below the cut, and in that group as the caller may read them now. They take one batch,
//! and the answer carries a ticket that undoes exactly that batch, sealed with the system's key
//! for this purpose and this person, and good for as long as an idempotency key replays an
//! answer. An ordinary read of a row, or its renewal, takes it out of the batch: an Undo then
//! leaves it as it is. Undo changes no thread's or chat's read position, no subscription, no
//! request and no decision.

orch_api_base::api_family_callback! {
    bind __api_bind_76181556db, __api_install_76181556db, __api_callback_76181556db;
    slot __ApiCallback76181556db, __API_CALLBACK_76181556DB, __api_set_76181556db;
    base orch_api_base::routes::notifications::grouping;

    async fn waiting_for<'__api>(arg0: &'__api orch_engine::Engine, arg1: &'__api mut orch_db::Conn, arg2: &'__api orch_authz::Principal, arg3: &'__api [orch_db::queries::feeds::InboxRow]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashSet<orch_types::Id>>;
}
