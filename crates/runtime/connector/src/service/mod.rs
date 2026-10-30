//! The per-user service that keeps a connector running.
//!
//! A systemd user unit on Linux, a launchd agent on macOS, a scheduled task at logon on Windows: what
//! `orch-connector service install` writes for the program that runs it and its data directory, and starts; the
//! installers write the same. None of it needs administrator rights. The platform's own data directory has the
//! plain name (`orch-connector.service`, `dev.orch.connector`, `Orch Connector`); a data directory that was named
//! gets a name of its own, suffixed with the first eight hexadecimal digits of the SHA-256 of its path as it was
//! written, as the installers name it, so that any number of connectors of one person run side by side.

pub mod linux;
pub mod macos;
pub mod windows;

use std::path::{Path, PathBuf};

/// What a service runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// The program, as an absolute path.
    pub program: PathBuf,
    /// Its data directory, as an absolute path.
    pub data_dir: PathBuf,
    /// The suffix of its name: none for the platform's own data directory.
    pub suffix: Option<String>,
    /// The variables it runs with besides its manager's own: the proxy it reaches out through
    /// ([`proxy_environment`]).
    pub environment: Vec<(String, String)>,
}

impl Service {
    /// The service of `program` with `data_dir`, which was `named` on the command line or is the platform's own.
    #[must_use]
    pub fn new(program: PathBuf, data_dir: PathBuf, named: bool) -> Self {
        let suffix = named.then(|| suffix_of(&data_dir));
        let absolute = |path: PathBuf| std::path::absolute(&path).unwrap_or(path);
        Self {
            program: absolute(program),
            data_dir: absolute(data_dir),
            suffix,
            environment: Vec::new(),
        }
    }

    /// The same service, run with `environment` besides its manager's own variables.
    #[must_use]
    pub fn with_environment(mut self, environment: Vec<(String, String)>) -> Self {
        self.environment = environment;
        self
    }
}

/// The variables that name a proxy, as every program reads them.
const PROXY_VARIABLES: [&str; 8] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// The proxy a service reaches out through, as the variables it runs with.
///
/// A service manager starts its programs with its own variables, not those of the shell the person works in,
/// so the system proxy a person has in their shell is written into the service: the proxy variables `var` has
/// (the installing shell's), unless `proxy` names another. `proxy` is a proxy's address (`http://`,
/// `socks5://` or `socks5h://`, with `user:password@` where it asks for them), which then serves every
/// destination, or `none`, which keeps every host from any proxy, the machine's settings included. `no_proxy`
/// names the hosts reached directly (comma-separated names, addresses and networks), in place of the shell's.
pub fn proxy_environment(
    var: impl Fn(&str) -> Option<String>,
    proxy: Option<&str>,
    no_proxy: Option<&str>,
) -> Result<Vec<(String, String)>, ServiceError> {
    let pair = |name: &str, value: &str| (name.to_owned(), value.to_owned());
    let mut environment: Vec<(String, String)> = match proxy.map(str::trim) {
        None => PROXY_VARIABLES
            .iter()
            .filter_map(|name| {
                var(name)
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| pair(name, &value))
            })
            .collect(),
        Some("none") => vec![pair("NO_PROXY", "*"), pair("no_proxy", "*")],
        Some(address) => {
            let url = url::Url::parse(address).map_err(|_| {
                ServiceError::Proxy(format!(
                    "`{address}` is not a proxy's address; write it as http://host:port, socks5://host:port or                      socks5h://host:port"
                ))
            })?;
            if !matches!(url.scheme(), "http" | "socks5" | "socks5h") || url.host_str().is_none() {
                return Err(ServiceError::Proxy(format!(
                    "the connector speaks to http://, socks5:// and socks5h:// proxies, with a host; `{}://` is not one",
                    url.scheme()
                )));
            }
            // An HTTP proxy serves every program by every variable. A SOCKS one is named only where programs
            // read it for every protocol (`ALL_PROXY`, as curl, git and the connector do): programs that read
            // `HTTPS_PROXY` alone and speak no SOCKS go on as they would without it.
            let names: &[&str] = if url.scheme() == "http" {
                &PROXY_VARIABLES[..6]
            } else {
                &["ALL_PROXY", "all_proxy"]
            };
            let mut set: Vec<(String, String)> =
                names.iter().map(|name| pair(name, address)).collect();
            if let Some(kept) = var("NO_PROXY").or_else(|| var("no_proxy")) {
                set.push(pair("NO_PROXY", &kept));
                set.push(pair("no_proxy", &kept));
            }
            set
        }
    };
    if let Some(hosts) = no_proxy.map(str::trim).filter(|hosts| !hosts.is_empty()) {
        environment.retain(|(name, _)| !name.eq_ignore_ascii_case("NO_PROXY"));
        environment.push(pair("NO_PROXY", hosts));
        environment.push(pair("no_proxy", hosts));
    }
    Ok(environment)
}

/// The platform service's actual name, shared by commands, reports and the page.
#[must_use]
pub fn name(service: &Service) -> String {
    if cfg!(target_os = "windows") {
        windows::task_name(service)
    } else if cfg!(target_os = "macos") {
        macos::label(service)
    } else {
        linux::unit_name(service)
    }
}

/// The suffix of a named data directory's service: the first eight hexadecimal digits of the SHA-256 of its
/// path as text, which is what the installers write.
#[must_use]
pub fn suffix_of(data_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = hex::encode(Sha256::digest(data_dir.to_string_lossy().as_bytes()));
    digest[..8].to_owned()
}

/// What a person asks of the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Write it and start it; one written before is written again and started again.
    Install,
    /// Stop it and remove it.
    Uninstall,
    /// Start it.
    Start,
    /// Stop it.
    Stop,
    /// Say whether it runs.
    Status,
}

/// Why the service could not be set up or asked.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// A file could not be written or removed.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What failed.
        #[source]
        source: std::io::Error,
    },
    /// The platform's service manager refused, or is not there.
    #[error("{0}")]
    Manager(String),
    /// The proxy it was given is not one the connector speaks to.
    #[error("{0}")]
    Proxy(String),
}

/// Does `action` for `service` on this platform, and says what came of it.
pub fn act(action: Action, service: &Service) -> Result<String, ServiceError> {
    if cfg!(target_os = "macos") {
        macos::act(action, service)
    } else if cfg!(target_os = "windows") {
        windows::act(action, service)
    } else {
        linux::act(action, service)
    }
}

/// Whether a service for `service`'s data directory is installed on this platform.
#[must_use]
pub fn installed(service: &Service) -> bool {
    if cfg!(target_os = "macos") {
        macos::installed(service)
    } else if cfg!(target_os = "windows") {
        windows::installed(service)
    } else {
        linux::installed(service)
    }
}

/// Runs a service manager's command and gives what it printed; its failure, with what it said, where it fails.
fn manager(program: &str, args: &[&str]) -> Result<String, ServiceError> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| ServiceError::Manager(format!("{program} could not be run: {error}")))?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
    if output.status.success() {
        return Ok(text(&output.stdout));
    }
    let said = text(&output.stderr);
    Err(ServiceError::Manager(format!(
        "{program} {}: {}",
        args.join(" "),
        if said.is_empty() {
            text(&output.stdout)
        } else {
            said
        }
    )))
}

/// Writes `bytes` to `path`, its directory made where it is not.
fn write(path: &Path, bytes: &[u8]) -> Result<(), ServiceError> {
    let io = |source| ServiceError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    std::fs::write(path, bytes).map_err(io)?;
    // A proxy's address can hold its password: the file is its owner's alone to read.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
    }
    Ok(())
}

/// Removes `path`; whether it was there.
fn remove(path: &Path) -> Result<bool, ServiceError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ServiceError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// The home directory, from the environment.
fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The installers' directories are Unix paths; Windows joins the same names with its own separator.
    #[cfg(unix)]
    #[test]
    fn a_named_directory_is_suffixed_as_the_installers_suffix_it_and_the_platforms_own_is_not() {
        // printf '%s' /srv/orch-a | sha256sum | cut -c1-8
        assert_eq!(suffix_of(Path::new("/srv/orch-a")), "3ef2a1c7");
        let named = Service::new("/opt/orch-connector".into(), "/srv/orch-a".into(), true);
        assert_eq!(named.suffix.as_deref(), Some("3ef2a1c7"));
        let own = Service::new("/opt/orch-connector".into(), "/srv/orch-a".into(), false);
        assert_eq!(own.suffix, None);
        assert_eq!(own.data_dir, PathBuf::from("/srv/orch-a"));
    }

    fn shell(names: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            names
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    fn pair(name: &str, value: &str) -> (String, String) {
        (name.to_owned(), value.to_owned())
    }

    #[test]
    fn the_service_keeps_the_proxy_of_the_shell_that_installs_it() {
        let environment = proxy_environment(
            shell(&[
                ("https_proxy", "http://proxy.corp:3128"),
                ("NO_PROXY", ".corp"),
                ("HTTP_PROXY", " "),
            ]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            environment,
            [
                pair("https_proxy", "http://proxy.corp:3128"),
                pair("NO_PROXY", ".corp")
            ]
        );
        assert!(
            proxy_environment(shell(&[]), None, None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_proxy_it_is_given_serves_every_destination_and_none_keeps_every_host_from_any() {
        let environment = proxy_environment(
            shell(&[
                ("HTTPS_PROXY", "http://old:3128"),
                ("no_proxy", "localhost"),
            ]),
            Some("socks5h://ada:secret@proxy:1080"),
            None,
        )
        .unwrap();
        assert_eq!(
            environment,
            [
                pair("ALL_PROXY", "socks5h://ada:secret@proxy:1080"),
                pair("all_proxy", "socks5h://ada:secret@proxy:1080"),
                pair("NO_PROXY", "localhost"),
                pair("no_proxy", "localhost"),
            ],
            "a SOCKS proxy is named for every protocol only, and the shell's HTTP one is gone"
        );
        let http = proxy_environment(shell(&[]), Some("http://proxy.corp:3128"), None).unwrap();
        for name in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            assert!(
                http.contains(&pair(name, "http://proxy.corp:3128")),
                "{name}: {http:?}"
            );
        }
        let direct = proxy_environment(
            shell(&[("HTTPS_PROXY", "http://old:3128")]),
            Some("none"),
            None,
        )
        .unwrap();
        assert_eq!(direct, [pair("NO_PROXY", "*"), pair("no_proxy", "*")]);
        let hosts = proxy_environment(
            shell(&[("NO_PROXY", ".old")]),
            Some("http://proxy:3128"),
            Some(".corp, 10.0.0.0/8"),
        )
        .unwrap();
        assert!(hosts.contains(&pair("NO_PROXY", ".corp, 10.0.0.0/8")));
        assert!(!hosts.iter().any(|(_, value)| value == ".old"));
    }

    #[test]
    fn a_proxy_the_connector_does_not_speak_is_refused_before_anything_is_written() {
        for wrong in [
            "https://proxy:443",
            "socks4://proxy:1080",
            "proxy:3128",
            "http://",
        ] {
            assert!(
                matches!(
                    proxy_environment(shell(&[]), Some(wrong), None),
                    Err(ServiceError::Proxy(_))
                ),
                "{wrong}"
            );
        }
    }
}
