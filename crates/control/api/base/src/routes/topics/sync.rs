//! The state behind a topic's feed: the topic, its threads, the jobs about them and its announcements, or what changed after a sequence.
//!
//! A client that lost its place on the feed of a topic asks here for what it needs to be whole
//! again. The topic is always there. Its threads are all of those the caller may view, whatever the
//! number the client holds: a thread's own state is stamped on the feed of its thread and on the
//! tree of its org, and not on the topic's, so that the topic's number cannot say which of them
//! changed, and a client takes each row by the sequence it carries. The jobs about the threads are
//! those that are under way, or from a number on, those stamped on the feed after it, over or not,
//! for the threads the caller may view. The announcements are the last twenty that went to the
//! threads, or from a number on, those after it.
//!
//! What the state consists of is [`MEMBERS`], one list, and the only place that names the members.

orch_api_base::api_family_callback! {
    bind __api_bind_e57ac81d44, __api_install_e57ac81d44, __api_callback_e57ac81d44;
    slot __ApiCallbacke57ac81d44, __API_CALLBACK_E57AC81D44, __api_set_e57ac81d44;
    base orch_api_base::routes::topics::sync;

    async fn root_plan<'__api>(arg0: &'__api mut orch_db::Conn, arg1: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::key_operations::admission::Prepared>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1ea126ee80, __api_install_1ea126ee80, __api_callback_1ea126ee80;
    slot __ApiCallback1ea126ee80, __API_CALLBACK_1EA126EE80, __api_set_1ea126ee80;
    base orch_api_base::routes::topics::sync;

    async fn read_for<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_engine::Engine, arg3: chrono::DateTime<chrono::Utc>, arg4: orch_types::Id, arg5: orch_types::Seq, arg6: Option<orch_types::Seq>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::topics::TopicSync>;
}
