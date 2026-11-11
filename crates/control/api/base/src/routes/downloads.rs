//! Connector binaries and installers served by the system.
//!
//! A release keeps these files in its payload, in a directory of `connectors` beside the
//! `migrations` the configuration names, and they are served as they are, so that a user's machine
//! never needs a code host. The connector carries its release and its BLAKE3 hash in two headers,
//! which the installers compare with the manifest; the hash is that of the bytes that follow,
//! worked out as they are read and not taken from the manifest. A system that was not started from
//! a release, a development build, has no such directory, and says so plainly instead of
//! answering with nothing.

/// The manifest of the files of the payload.
pub const MANIFEST_FILE: &str = "version.json";

orch_api_base::api_family_callback! {
    bind __api_bind_e20b139d4b, __api_install_e20b139d4b, __api_callback_e20b139d4b;
    slot __ApiCallbacke20b139d4b, __API_CALLBACK_E20B139D4B, __api_set_e20b139d4b;
    base orch_api_base::routes::downloads;

    fn mount<'__api>(arg0: &'__api mut orch_api_base::registry::Registry) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_f377bccefd, __api_install_f377bccefd, __api_callback_f377bccefd;
    slot __ApiCallbackf377bccefd, __API_CALLBACK_F377BCCEFD, __api_set_f377bccefd;
    base orch_api_base::routes::downloads;

    fn payload_dir<'__api>(arg0: &'__api orch_config::ServerConfig) -> std::path::PathBuf;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1bf4f6bf88, __api_install_1bf4f6bf88, __api_callback_1bf4f6bf88;
    slot __ApiCallback1bf4f6bf88, __API_CALLBACK_1BF4F6BF88, __api_set_1bf4f6bf88;
    base orch_api_base::routes::downloads;

    fn connector_names<'__api>(arg0: &'__api str, arg1: &'__api str) -> [String; 3];
}
