//! A path of a workspace as a request writes it, and what it may be.
//!
//! A path is relative to the root of the environment's workspace and `/`-separated: no leading `/`,
//! no empty, `.` or `..` components, at most 4 096 bytes; the empty string is the root. A path that
//! leaves the workspace does not exist for anybody, and the request is answered as if there were
//! nothing there without the machine being asked. Windows separators are converted. What the
//! workspace's ignore rules leave out is the machine's to say, since the rules are applied where
//! the files are.

orch_api_base::api_family_callback! {
    bind __api_bind_4f59b857ad, __api_install_4f59b857ad, __api_callback_4f59b857ad;
    slot __ApiCallback4f59b857ad, __API_CALLBACK_4F59B857AD, __api_set_4f59b857ad;
    base orch_api_base::routes::workspace::paths;

    fn clean<'__api>(arg0: &'__api str, arg1: &'__api str, arg2: bool) -> orch_api_base::middleware::requests::ApiResult<String>;
}
