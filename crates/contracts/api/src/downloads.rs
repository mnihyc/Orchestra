//! Connector downloads and release manifest views.
//!
//! These requests are public and serve files of the release payload as they
//! are, so that a user's machine never needs a code host: the connector
//! binary for an operating system and a processor, the node of a host, the two
//! installers of a person's machine and the one of a host, and the release
//! manifest. Every answer is a file, so every reply is raw; the
//! connector carries its version and its BLAKE3 hash in two headers, which
//! the installers compare with the manifest.

use crate::routes::Access::Public;

/// The operating systems a connector is built for, as the `{os}` of its path.
pub const CONNECTOR_OS: [&str; 3] = ["linux", "macos", "windows"];
/// The processors a connector is built for, as the `{arch}` of its path.
pub const CONNECTOR_ARCH: [&str; 2] = ["x86_64", "aarch64"];
/// The header of a connector download that carries the release it belongs to.
pub const VERSION_HEADER: &str = "x-orch-version";
/// The header of a connector download that carries the BLAKE3 hash of the binary, in lower case hex.
pub const BLAKE3_HEADER: &str = "x-orch-blake3";

crate::endpoints! {
    /// Serves the connector binary for an operating system and a processor, with its version and its BLAKE3 hash in headers.
    DownloadConnectorGet: GET "/downloads/connector/{os}/{arch}", Public, none, none, raw, 200;
    /// Serves the POSIX installer.
    DownloadInstallShGet: GET "/downloads/install.sh", Public, none, none, raw, 200;
    /// Serves the PowerShell installer.
    DownloadInstallPs1Get: GET "/downloads/install.ps1", Public, none, none, raw, 200;
    /// Serves the installer of a host's node, which a host's enrollment command runs.
    DownloadInstallNodeShGet: GET "/downloads/install-node.sh", Public, none, none, raw, 200;
    /// Serves the node of a host, built for Linux, for a processor, with its version and its BLAKE3 hash in headers.
    DownloadNodeGet: GET "/downloads/node/{arch}", Public, none, none, raw, 200;
    /// Serves the release manifest, as the release wrote it.
    DownloadVersionGet: GET "/downloads/version.json", Public, none, none, raw, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::HarnessArch;
    use crate::resources::MachineOs;
    use serde_json::json;

    #[test]
    fn a_connector_is_offered_for_every_system_and_processor_the_contract_knows() {
        let systems = [MachineOs::Linux, MachineOs::Macos, MachineOs::Windows]
            .map(|os| serde_json::to_value(os).unwrap());
        assert_eq!(systems, CONNECTOR_OS.map(|os| json!(os)));
        let processors = [HarnessArch::X86_64, HarnessArch::Aarch64]
            .map(|arch| serde_json::to_value(arch).unwrap());
        assert_eq!(processors, CONNECTOR_ARCH.map(|arch| json!(arch)));
    }
}
