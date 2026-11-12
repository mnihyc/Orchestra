//! Entering a port without the browser route: a link's use, its landing, the raw socket and the public assertion keys.
//!
//! A person's helper gets a use of a port with their own sign-in (`POST …/ports/{port}/use`); a
//! link's holder gets one with the link (`POST /port-links/use`), by a way in the link has. Either
//! use is then carried over one WebSocket (`GET /ws/ports/{use_id}`): TCP bytes in binary messages,
//! the end of one direction as the text message `eof`, or one UDP datagram per binary message. A
//! link's landing is a generic page of the web application, which reads the proof from the
//! address's fragment and asks `POST /port-links/inspect` for what may be shown. The public keys
//! that verify a caller's assertion are served to anybody: they are public.

pub use orch_pki::port_assertions::PortAssertionSigner;

use std::sync::OnceLock;
pub static SIGNER: OnceLock<PortAssertionSigner> = OnceLock::new();

/// The port assertion key: the one given, or else one made now for this process alone, which can
/// no longer vouch for what it signed once the process ends (a deployment gives its own).
pub fn port_signer() -> &'static PortAssertionSigner {
    SIGNER.get_or_init(|| {
        tracing::warn!(
            "the installation gave no port assertion key; assertions made now cannot be verified after a restart"
        );
        PortAssertionSigner::generate()
    })
}

orch_api_base::api_family_callback! {
    bind __api_bind_36aa4772f8, __api_install_36aa4772f8, __api_callback_36aa4772f8;
    slot __ApiCallback36aa4772f8, __API_CALLBACK_36AA4772F8, __api_set_36aa4772f8;
    base orch_api_base::routes::port_entry;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_7dd1829dd2, __api_install_7dd1829dd2, __api_callback_7dd1829dd2;
    slot __ApiCallback7dd1829dd2, __API_CALLBACK_7DD1829DD2, __api_set_7dd1829dd2;
    base orch_api_base::routes::port_entry;

    fn assertion_keys<'__api>(arg0: &'__api orch_api_base::state::App) -> orch_api_contract::ports::PortAssertionKeys;
}
