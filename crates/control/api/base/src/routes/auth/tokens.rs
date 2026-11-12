//! API tokens made narrow and with a fresh proof, within the longest life their places admit, asked again by their key without their secret; and an org's list of the tokens that reach it, each of which it can refuse there.
//!
//! A token made without scopes may use nothing: the page's presets send an
//! explicit list, and everything its person may only when `*` is chosen. The
//! request that makes it carries the maker's fresh proof ([`fresh`]). Its whole
//! life, from now to its end, is checked against `tokens.max_days` where it is
//! made: the system's for a personal token, the orgs down to its node or to the
//! service identity's org as well; an org it reaches later is checked when it
//! is used (`authz` keeps it out). A creation asked again with the same
//! `Idempotency-Key` answers what was made, with no secret: none is kept to be
//! given twice. An org's members managers list the tokens that can reach it and
//! refuse one there, which keeps it out of the org and everything inside from
//! the next request on, and changes nothing elsewhere.
//!
//! [`fresh`]: crate::sign_in::fresh

orch_api_base::api_family_callback! {
    bind __api_bind_ff99c0b88b, __api_install_ff99c0b88b, __api_callback_ff99c0b88b;
    slot __ApiCallbackff99c0b88b, __API_CALLBACK_FF99C0B88B, __api_set_ff99c0b88b;
    base orch_api_base::routes::auth::tokens;

    fn scope_info(arg0: Option<orch_types::Json>) -> orch_api_base::middleware::requests::ApiResult<Option<orch_access_contract::key_scope::KeyScope>>;
}
